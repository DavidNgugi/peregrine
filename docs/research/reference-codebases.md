# Reference codebase teardown for Peregrine

Method: GitHub REST API for metadata/license/commits, upstream tarballs (`codeload.github.com`) extracted to `/tmp/pereg` for source reading, `grep`/`read` over real files. LOC = counted `\n` + 1 locally (includes comments and blanks). All four snapshots taken 2026-10-08/09 UTC.

| Repo | Purpose | SPDX | Language | LOC (measured) | Stars | Last commit |
|---|---|---|---|---|---|---|
| seerr-team/seerr | media request/discovery server for Jellyfin/Plex/Emby | MIT | TypeScript (Next.js + Express) | 49,213 `server/*.ts`, 52,809 `src/**/*.ts(x)`, 957 cypress | 12,839 | 2026-10-08T21:27:50Z `9e2021b829` |
| videolan/vlc | media player + libvlc engine | GPL-2.0 (app) / LGPL-2.1+ (libvlc) | C/C++/ObjC/QML | ~1,128,070 (c 620,214 / cpp 175,417 / h 145,452 / m 90,905 / hpp 49,621 / qml 43,899 / mm 2,562), excl. `contrib/`, `po/` | 19,914 | 2026-10-03T18:41:31Z `f77762543b` |
| Aayush9029/Peeri | native macOS torrent/download client (aria2 front-end) | MIT | Swift 5 / SwiftUI | 7,136 Swift (6,514 non-test; `Peeri/` 4,786, `PeerKit/Sources` 1,627) | 46 | 2026-09-12T20:21:22Z `e0c5a4cc6b` |
| baairon/torlink | terminal torrent finder/downloader | MIT | **TypeScript** (not Go), Ink + React TUI | 15,069 in `src/` (9,258 non-test) | 5,853 | 2026-10-07T18:08:03Z `210387185b` |

License verification (all `verified:` against the repo's own license file via `api.github.com/repos/<r>/license` and the raw file):
seerr MIT `verified: https://github.com/seerr-team/seerr/blob/develop/LICENSE` (sha `c188c311169523e2e22f3494a55370fb605360d2`);
VLC `verified: https://github.com/videolan/vlc/blob/master/README.md` (lines 20-26: "VLC is released under the GPLv2 (or later)"; "libVLC, the engine is released under the LGPLv2 (or later)") and `COPYING`(GPL-2.0)/`COPYING.LIB`(LGPL-2.1) in-repo;
Peeri MIT `verified: https://github.com/Aayush9029/Peeri/blob/main/LICENSE` (sha `113c8a9724b8d90f3f7aacc9d74a4ea0efe118ef`);
torlink MIT `verified: https://github.com/baairon/torlink/blob/main/LICENSE` (sha `4e5683380f8d38cd53303abc2cbb8171ed78c3b8`).
Caveat: GitHub's `license.spdx_id` returns `GPL-2.0` for VLC, which is the *app* license; libVLC is separately LGPL-2.1+ per README. Both were read directly.

---

## 1. seerr (seerr-team/seerr)

### Purpose / stack
Self-hosted request + discovery server: users request movies/TV, admins approve, requests are pushed to Radarr/Sonarr, availability is synced back from Plex/Jellyfin/Emby. MIT `verified: https://github.com/seerr-team/seerr/blob/develop/LICENSE`. Stack (`package.json`): Next.js 16.2.6, React 19.2.6, SWR 2.4.1, @headlessui/react, Express 5.2.1, TypeORM 0.3.31, pg 8.23, node-schedule 2.1.1, winston, zod 4.3.6, pnpm; a hand-written OpenAPI spec `seerr-api.yml` (8,307 lines) drives the public docs.

### Architecture
- Two processes in one repo: `server/` (Express API, `server/index.ts`) and `src/` (Next.js UI). `server/routes/index.ts` composes routers; `server/entity/*` are TypeORM entities; `server/subscriber/*` are ORM subscribers; `server/job/schedule.ts` registers cron jobs with `node-schedule`; `server/lib/scanners/*` are per-media-server scanners.
- Data flow: UI → `/api/v1/*` → route → `MediaRequest` entity → `MediaRequestSubscriber` (`afterInsert`/`afterUpdate`) → `sendToRadarr`/`sendToSonarr` → scanners + `availabilitySync` pull availability back → `Media.status`/`Season.status` → notifications.
- Abstraction boundaries: `ExternalAPI` (HTTP + cache + rate limit), `CacheManager` (per-provider LRU tiers), `ImageProxy` (disk cache), `TvShowProvider` (metadata), notification `NotificationAgent`.

### Metadata provider layer (TMDB/TVDB)
- Single base class `server/api/externalapi.ts`: wraps axios, injects cache + rate limit + proxy/UA interceptors; `get()` is cache-aside with per-call TTL, `getRolling()` is stale-while-revalidate (`externalapi.ts:112-145`).
- TMDB client `server/api/themoviedb/index.ts` (1,497 lines) is constructed with `nodeCache: cacheManager.getCache('tmdb').data` and `rateLimit: { maxRequests: 20, maxRPS: 50 }` (`themoviedb/index.ts:186-196`).
- The provider abstraction is `server/api/provider.ts` `interface TvShowProvider { getTvShow, getTvSeason, getShowByTvdbId }` — but it returns *TMDB-shaped* types (`TmdbTvDetails`, `TmdbSeasonWithEpisodes`). `class Tvdb extends ExternalAPI implements TvShowProvider` (`server/api/tvdb/index.ts:42`) delegates to TMDB for the base object and layers TVDB data on top (`tvdb/index.ts:168-194`). Provider choice is a setting: `MetadataProviderType.TMDB | TVDB` for `tv` and `anime` (`server/lib/settings/index.ts:111-116`).
- Gotcha in the cache key: `serializeCacheKey` is `` `${baseUrl}${endpoint}${JSON.stringify(options)}` `` (`externalapi.ts:154-163`) — key order and object key order matter, so semantically identical requests with reordered params miss.

### Image proxying / caching
- `server/lib/imageproxy.ts` (357 lines) is a **disk** cache (not memory, not DB): `<CONFIG_DIRECTORY>/cache/images/<key>/<maxAge>.<expireAt>.<etag>.<ext>`; cache key = `sha256(key + cacheVersion + path)` base64 with `/`→`-` (`imageproxy.ts:318-354`).
- Stale entries are served immediately and revalidated in the background (`imageproxy.ts:181-185`). The response tells the client exactly what happened:
```ts
res.writeHead(200, {
  'Content-Type': `image/${imageData.meta.extension}`,
  'Cache-Control': `public, max-age=${imageData.meta.curRevalidate}`,
  'OS-Cache-Key': imageData.meta.cacheKey,
  'OS-Cache-Status': imageData.meta.cacheMiss ? 'MISS' : 'HIT',
});
```
- Route `server/routes/imageproxy.ts` hosts `https://image.tmdb.org` and `https://artworks.thetvdb.com` behind `ImageProxy` instances, each rate-limited to `maxRequests: 20, maxRPS: 50`, and rejects `//`/`://` in the path before proxying (`routes/imageproxy.ts:44-48`).

### Cache tiers (`server/lib/cache.ts`)
In-memory `lru-cache` per provider; **key-count** bounds, not byte bounds, with a comment saying why (`cache.ts:21-49`): `tmdb` stdTtl 21,600s / 1000 keys, `tmdbscan` 900s / 2000, `radarr`/`sonarr` 64, `rt` 500, `plextv` 1 week / 5000, `tvdb` 21,600s / 500. `get()` returns `structuredClone(entry.value)` because callers mutate results in place (`cache.ts:126-138`). Cache stats (`hits`, `misses`, `keys`, `ksize`, `vsize`) are exposed for the admin UI.

### Request workflow, notifications, availability
- Status enums are the whole contract (`server/constants/media.ts`):
  - `MediaRequestStatus { PENDING=1, APPROVED, DECLINED, FAILED, COMPLETED }`
  - `MediaStatus { UNKNOWN=1, PENDING, PROCESSING, PARTIALLY_AVAILABLE, AVAILABLE, BLOCKLISTED, DELETED }`
- Approval mutates the request to `APPROVED` (`server/routes/request.ts:769,805-806`); the ORM subscriber then performs the side effects:
```ts
public async afterUpdate(event: UpdateEvent<MediaRequest>): Promise<void> {
  if (!event.entity) { return; }
  try {
    await this.sendToRadarr(event.entity as MediaRequest, event.manager);
    await this.sendToSonarr(event.entity as MediaRequest, event.manager);
  } catch (e) { logger.error('Error while sending to *arr in afterUpdate subscriber', ...); }
```
(`server/subscriber/MediaRequestSubscriber.ts:1055-1069`; `sendToRadarr` at `:184`, `sendToSonarr` at `:483`; the file is 1,142 lines.)
- Notifications: bitmask `enum Notification` (`MEDIA_PENDING=2, MEDIA_APPROVED=4, MEDIA_AVAILABLE=8, MEDIA_FAILED=16, MEDIA_DECLINED=64, MEDIA_AUTO_APPROVED=128, ISSUE_*`…) and a plugin list in `server/lib/notifications/agents/`: discord, email, gotify, ntfy, pushbullet, pushover, slack, telegram, webhook, webpush. Dispatch is fire-and-forget over registered agents: `this.activeAgents.forEach((agent) => { if (agent.shouldSend()) agent.send(type, payload); })` (`lib/notifications/index.ts:109-111`).
- Availability is recomputed by a 1,218-line reconciler (`server/lib/availabilitySync.ts`) that compares `Media.seasons[]` against Sonarr seasons / Plex / Jellyfin results and writes `DELETED` back (`availabilitySync.ts:616-624`), scheduled through `server/job/schedule.ts`.

### DB schema for shows/seasons/episodes
- **There is no `Episode` entity.** `server/entity/` contains Media, MediaRequest, Season, SeasonRequest, User, Watchlist, Issue, Blocklist, OverrideRule, DiscoverSlider, Session, UserSettings, UserPushSubscription — no Episode. Episodes are never persisted; episode lists are fetched from TMDB on demand and availability is stored per season.
- `Media` (`server/entity/Media.ts`): `@Index(['tmdbId','mediaType'])` plus separate indexes on `tmdbId`, `tvdbId` (`unique`, nullable), `imdbId`, `status`, `status4k`; `@OneToMany(() => Season, ..., { cascade: true, eager: true })` — every Media load eager-fetches all Seasons (`Media.ts:34-155`).
- `Season` (`server/entity/Season.ts`, 47 lines total) is the entire per-season state:
```ts
@Entity()
class Season {
  @PrimaryGeneratedColumn() public id: number;
  @Column() public seasonNumber: number;
  @Column({ type: 'int', default: MediaStatus.UNKNOWN }) public status: MediaStatus;
  @Column({ type: 'int', default: MediaStatus.UNKNOWN }) public status4k: MediaStatus;
  @ManyToOne(() => Media, (media) => media.seasons, { onDelete: 'CASCADE' })
  @Index() public media: Promise<Media>;
```
- 4K is a parallel column (`status` vs `status4k`) all the way down, not a row per quality.
- Migrations: TypeORM, 55 files in `server/migration/sqlite/` (+ a `postgres/` twin) generated via `pnpm migration:generate`; `server/utils/DbColumnHelper.ts` (`DbAwareColumn`, `resolveDbType`) abstracts SQLite vs Postgres.
- Settings are a single JSON file (`config/settings.json`) held in memory with a serialized `saveLock: Promise<void>` (`server/lib/settings/index.ts:397-402`), containing `main`, `plex`, `jellyfin`, `radarr[]`, `sonarr[]`, `notifications.agents`, `metadataSettings`, `public`.

### Concrete lessons to take
- Model availability as `(media, season)` with two status columns (hd/4k) and never persist episodes; derive episode lists from the metadata provider at read time. Maps directly to a Rust `media`/`season` table plus a provider trait.
- `ExternalAPI` + `CacheManager` + `ImageProxy` is a clean three-tier separation (HTTP semantics / JSON cache / binary cache). Port the shape, not the code: one trait `MetadataProvider` with cache-aside and rate limit wrapping.
- Cache tiers keyed by provider with explicit *key* ceilings and a written rationale per tier is cheap insurance against unbounded growth; expose hit/miss/key counters.
- Stale-while-revalidate for image and metadata reads keeps the UI instant; `getRolling` is ~20 lines and worth taking conceptually.
- Generic provider interface (`TvShowProvider`) with an adapter per backend is the right boundary — but note that seerr's interface leaks TMDB's types, which is why TVDB had to call TMDB internally.
- Bitmask notification enums + a list of agents with `shouldSend()`/`send()` makes adding a channel a new file only. Peregrine's equivalent: a `Notifier` trait with a registry.
- `X-Proxy-Cache: HIT|MISS`-style response headers (`OS-Cache-Status`) make cache behaviour observable from the client without a debugger.
- `structuredClone` on cache reads (callers mutate) — in Rust, return `Arc<T>` or clone deliberately; the bug class (shared mutable cached value) is real.
- Migrations are generated per DB dialect from one entity model; keep sqlite for desktop and postgres for a future server with a column-type helper.
- 8,307-line OpenAPI spec as the single API contract, checked in and used to generate docs (`gen-docs/`), is how you keep a Tauri UI and a companion server honest.

### Concrete mistakes / limits to avoid
- Network I/O inside an ORM subscriber (`afterUpdate` → Radarr/Sonarr HTTP) couples DB transactions to a slow third party; failures are logged and swallowed (`MediaRequestSubscriber.ts:1055-1069`). Use an outbox/queue in Peregrine.
- `eager: true` on `Media.seasons` loads every season for every media fetch; at library scale this is the classic N+1-shaped payload problem.
- A 1,142-line subscriber class that both pushes to Radarr/Sonarr *and* sends ~8 notification types is a god object; split orchestration from persistence.
- Cache keys built with `JSON.stringify` (order-sensitive) silently miss; use a canonical key (sorted params / typed struct → hash).
- `tvdb` provider depends on `tmdb` internally (`tvdb/index.ts:176`), so "choose your provider" is not a real isolation boundary — a failure in TMDB breaks the TVDB path.
- No `Episode` table means per-episode request/availability state cannot be expressed except by adding `SeasonRequest` rows; seerr itself needed `SeasonRequest` as a patch. If Peregrine wants per-episode "downloaded this one" state, model it up front.

---

## 2. VLC (videolan/vlc)

### Purpose / stack
Reference media player and the libvlc embeddable engine. Mirror snapshot: 6,299 files, 30 module directories, 19,914 stars, `pushed_at` 2026-10-07, default branch `master`. Note the GitHub repo is a mirror of `code.videolan.org/videolan/vlc` and can lag.

### Architecture (the pipeline worth understanding)
Pipeline: **access → stream → demux → es_out → decoder → vout/aout**, with modules discovered by a capability registry.
- Access modules produce a byte stream: `modules/access/file.c`, `modules/access/{http,ftp,sftp,bluray,dvdread,…}.c`.
- `src/input/stream.c` gives buffered, seekable, peekable IO: `vlc_stream_ReadBlock` (`:585`), `vlc_stream_Seek` (`:649`, seek-inside-peek-buffer fast path), capability flags `STREAM_CAN_SEEK | CAN_FASTSEEK | CAN_PAUSE | CAN_CONTROL_PACE` (`:762-792`). This is the byte-range abstraction a torrent streamer needs.
- `src/input/demux.c` creates demuxers: `demux_t *demux_New(vlc_object_t *, const char *module, const char *url, stream_t *s, es_out_t *out)` (`demux.c:73`). Demuxers register as a capability with a priority:
```c
vlc_module_begin ()
    set_description( N_("MP4 stream demuxer") )
    set_capability( "demux", 240 )
    set_callbacks( Open, Close )
vlc_module_end ()
```
(`modules/demux/mp4/mp4.c:72-75`; same pattern in `modules/stream_out/chromecast/chromecast_demux.cpp:496-503` with capability `"demux_filter"`.)
- `src/input/es_out.c` is the elementary-stream selector; decoders are `spu decoder`/`video decoder`/`audio decoder` capabilities.
- Playback control is `src/input/input.c` + `include/vlc_player.h`; libvlc is a thin wrapper (`lib/media_player.c`).

### Parts relevant to Peregrine
| Concern | Where | Note |
|---|---|---|
| libvlc embedding | `include/vlc/libvlc*.h`, `lib/*.c`, `libvlc.sym` | LGPL-2.1+ headers; symbol export list is a real ABI contract |
| Demux/streaming | `src/input/{stream,demux,access}.c`, `modules/access/*`, `modules/demux/*` | reference model for seek/peek/backpressure |
| Subtitle decode/render | `modules/codec/subsdec.c` (text, prio 50), `modules/codec/libass.c` (ASS, prio 100), `modules/demux/subtitle.c` (parser, capability `demux`, `:68-76`), `modules/text_renderer/freetype/freetype.c`, `include/vlc_subpicture.h` | text subtitle decode and ASS layout are genuinely reusable ideas |
| Casting/DLNA | `modules/stream_out/chromecast/*` (esp. `chromecast_ctrl.cpp`, `cast_channel.proto`), `modules/stream_out/dlna/dlna.cpp`, discovery in `modules/services_discovery/{upnp.cpp,microdns.c}` | renderer discovery is separate from stream-out |
| HW decode | `include/vlc_codec.h` (`decoder_t.get_device` `:52`, `enum vlc_decoder_device_type` `:593-601` incl. `VLC_DECODER_DEVICE_VIDEOTOOLBOX`), `include/vlc_fourcc.h:498-502` (`VLC_CODEC_CVPX_*`), `modules/codec/videotoolbox/{decoder,dpb,pacer}.c`, `modules/hw/{vaapi,vdpau,d3d11,nvdec}` | decoder-device negotiation is the pattern to copy conceptually |
| Audio output | `modules/audio_output/apple/auhal.c`, `coreaudio_common.c`; `wasapi.c`, `pulse.c`, `pipewire.c` | device-change callbacks exist in the 4.0 API |
| Subtitle model | `include/vlc_subpicture.h` `subpicture_region_t` (`:69`, carries `text_segment_t *p_text` and max width/height), `subpicture_t` (`:244`, channel id, start/stop dates, region list); render/merge in `src/video_output/vout_subpictures.c` (`spu_RenderSubpictures`, `:778`) | "text regions rendered at composite time" beats baking subtitles into frames |

Also relevant: VLC 4.0 libvlc has **no event manager**. `libvlc_event_manager_t` / `libvlc_event_attach` were deleted on master:
`lib: remove libvlc_event_manager_t` — https://github.com/videolan/vlc/commit/2562530b42 (2023-05-04). `include/` contains zero `libvlc_event` references today. The 4.0 model is a versioned callback struct passed at construction:
```c
LIBVLC_API libvlc_media_player_t *
libvlc_media_player_new( libvlc_instance_t *p_libvlc_instance,
                         const struct libvlc_media_player_cbs *cbs,
                         void *cbs_opaque );
```
(`include/vlc/libvlc_media_player.h:650-654`; struct starts with `uint32_t version;` at `:230-243`.) Every blog post showing `libvlc_event_attach` describes VLC 3.x.

### Licensing warning (read before copying anything)
- **libVLC is LGPL-2.1-or-later.** `verified: https://github.com/videolan/vlc/blob/master/README.md` — "libVLC, the engine is released under the LGPLv2 (or later). This allows embedding the engine in 3rd party applications, while letting them to be licensed under other licenses." File-level check by license header across the tree: `lib/` 23 LGPL / 0 GPL, `include/` 127 LGPL / 0 GPL, `src/` 304 LGPL / 2 GPL, `modules/` 1,418 LGPL / **1,198 GPL** (541 of them under `modules/gui/`).
- **The `vlc` application is GPL-2.0-or-later.** `verified: https://github.com/videolan/vlc/blob/master/COPYING` and README line 22. On some platforms it is effectively GPLv3 via dependencies (README line 23).
- Consequences for Peregrine:
  - **Copying any VLC source file into our tree** makes that file (and derivatives) LGPL-2.1+ if it carries the LGPL header, and GPL-2.0+ if it carries the GPL header → the GPL files are off-limits for a non-GPL app, and even the LGPL ones carry notice/relink obligations. **Do not port VLC code.**
  - **Dynamically linking libvlc** (a shared library the user installs, or we ship as an unmodified separate binary) is the intended LGPL use and does not relicense our code, provided we ship the LGPL text, keep libvlc replaceable (no static link), and offer libvlc's source. Static linking libvlc would require providing relinkable objects — avoid.
  - **Launching the `vlc` binary as a separate process** (like Peeri hands a file to the OS default app) is the cleanest route: GPL obligations attach to VLC's own distribution, not to Peregrine.
  - Recommendation per area: libvlc embedding → **separate process, or optional runtime-loaded dependency (LGPL-compliant dynamic link) only**; demux/streaming, subtitle pipeline, decoder-device negotiation, chromecast/DLNA → **reference only, re-implement** (FFmpeg + Rust crates give us the same primitives under our own license terms); no VLC file may be copied, and nothing from `modules/gui/` or any GPL-header file may be read as a source of code to transplant.
- Practical: also audit codec patent/royalty exposure separately from copyright — hw decode on macOS via VideoToolbox is the low-risk path VLC itself takes (`VLC_DECODER_DEVICE_VIDEOTOOLBOX`).

### Concrete lessons to take
- Split the media path into explicit stages (access → stream → demux → es_out → decoder → out) and make each stage a trait with capability priorities; this is exactly how `demux` prio 240 vs HEIF prio 239 is resolved.
- Model the byte source as `read / peek / seek / can_seek / can_pause / can_control_pace`. Torrent streaming needs all six concepts; VLC's `vlc_stream_Seek` peek-buffer fast path is a directly useful idea.
- `demux_filter` as a capability (used by the Chromecast wrapper) is a neat way to insert a re-muxing stage without forking the demuxer.
- Versioned callback structs (`uint32_t version` first field) are how you evolve a C ABI without breaking embedders — the same trick applies to our Tauri↔Rust IPC surface and any C ABI we expose.
- Subtitles are a *region list with timing and layout*, not burned pixels: keep `text_segment_t`/`subpicture_region_t`-style structured subtitles, render at composite time, and support both plain-text and ASS sources with priorities (libass prio 100 > subsdec prio 50).
- Keep renderer discovery (`libvlc_renderer_discoverer.h`, upnp/microdns) decoupled from stream output (chromecast/dlna modules) so casting is optional.
- Decoder-device negotiation (`decoder_t.get_device` + `enum vlc_decoder_device_type` + `VLC_CODEC_CVPX_*` fourccs) is the abstraction that lets one decode path target VideoToolbox/D3D11/VAAPI/NVDEC.
- Audio output as a swappable module with `on_audio_device_changed` in the callback struct: device hot-plug is a first-class event, not a retry loop.

### Concrete mistakes / limits to avoid
- Do not follow the GitHub mirror for API truth: it is a mirror of `code.videolan.org`, and `master` has already removed the event API everyone documents (commit `2562530b42`). Any tutorial-based integration for VLC 4.0 is wrong.
- Do not attempt partial adoption of VLC modules: the LGPL/GPL split runs *through* `modules/` (1,198 GPL files), so "just grab the demuxer" is not safe.
- The pipeline's maturity comes from C-era error handling (`block_t`, manual refcounting, `vlc_tick_t`); reimplementing the shape in Rust is fine, but porting idioms is a downgrade.
- Subtitles depend on fontconfig/system fonts (`modules/text_renderer/freetype/platform_fonts.c`) — a cross-platform app inherits that dependency unless font provisioning is solved explicitly.
- Chromecast support leans on protobuf + a cast-channel implementation (`stream_out/chromecast/cast_channel.proto`); casting is not a small feature and should be a later milestone.

---

## 3. Peeri (Aayush9029/Peeri)

### Purpose / stack
Native macOS SwiftUI client that wraps a bundled `aria2c` daemon (BitTorrent + HTTP) and `yt-dlp` (YouTube/Vimeo). MIT `verified: https://github.com/Aayush9029/Peeri/blob/main/LICENSE`. 46 stars, 1 fork, repo size 86,779 KB, `pushed_at` 2026-09-12.

### Architecture
- `Peeri/` = SwiftUI app (4,786 lines): `AppUIModel`, `ContentView`, `Views/*`; `PeerKit/` = SwiftPM package (1,627 lines) with modules `Aria2Kit`, `Models`, `Shared`, `UI`, `Assets`.
- `Peeri/Services/Aria2DaemonManager.swift` launches the bundled binary as a child process; `Aria2DaemonManager.swift:25-36`:
```swift
guard let executable = Bundle.main.url(forResource: "aria2c", withExtension: nil) else {
    throw CocoaError(.fileNoSuchFile)
}
let root = FileManager.default.homeDirectoryForCurrentUser.appending(path: ".peeri/aria2")
...
task.arguments = [
    "--conf-path=\(config.path)", "--input-file=\(session.path)",
    "--save-session=\(session.path)", "--save-session-interval=5",
    "--force-save=true", "--stop-with-process=\(ProcessInfo.processInfo.processIdentifier)"
]
```
- `Peeri/Services/DownloadManager.swift` (664 lines) is the single `@MainActor @Observable` state holder; it polls `updateDownloads()` every 1s (`:177-186`), talks JSON-RPC at `http://localhost:16800/jsonrpc` with `rpc-secret "peeri"` (`:67-69`), and persists the list to `~/Documents/peeri/downloads/downloads.json` via `@Shared(.fileStorage(...))` (`:17-23`).
- Settings → `aria2.conf` rendering is one pure function: `PeeriSettings.toAria2ConfigString(logPath:)` (`PeerKit/Sources/Models/PeeriSettings.swift:145-165`) plus `toAria2DownloadOptions()` (`:178-194`).
- Boundary: Peeri owns UI + settings + persistence; **aria2 owns all transfer semantics**.

### Torrent search
**None.** Verified: no torrent index/client integration exists (`grep -rn "torrentz|1337|thepiratebay|apibay|searchTorrent" --include=*.swift` → 0 hits). Input is a magnet URL, an `http(s)` URL, or a `.torrent` file (`DownloadManager.swift:190-238`). The command palette searches *commands*, not torrents.

### Magnet resolution
Delegated to aria2: `aria2Client.addUri(url, options)` for URLs/magnets and `addTorrent(base64, [], options)` for `.torrent` files (`DownloadManager.swift:204,230`); `Aria2Kit/Aria2RPCClient.swift:72-87`. For display, a magnet is *reconstructed* from the status: info hash plus `bittorrent.announceList` (`Aria2RPCClient.swift:246-252`):
```swift
if let infoHash = status.infoHash {
    var components = URLComponents()
    components.scheme = "magnet"
    components.queryItems = [URLQueryItem(name: "xt", value: "urn:btih:" + infoHash)]
        + (status.bittorrent?.announceList ?? []).flatMap { $0 }.map { URLQueryItem(name: "tr", value: $0) }
```
Torrent name/file paths come from `status.bittorrent.info.name` and `status.files[].path` (`:260-283`).

### Sequential/streaming download, buffering, readahead
- **Not sequential, and not streaming.** aria2's piece order is exposed as four options (`PeerKit/Sources/Models/Aria2Preferences.swift:31-37`):
```swift
var options = [
    "stream-piece-selector": ["default", "inorder", "random", "geom"].contains(pieceOrder) ? pieceOrder : "default",
    "bt-prioritize-piece": prioritizePreview ? "head=4M,tail=4M" : "",
    ...
```
  i.e. an explicit `inorder` option exists but is *not* the default, and the "preview" feature prioritizes only head and tail.
- The README is explicit: "Torrent file-preview priority downloads the beginning and end first; it is not strict sequential torrent downloading" (`README.md`).
- There is no readahead/buffer manager, no local HTTP media server, and no embedded player. "Open" hands the finished **file path** to macOS:
```swift
func openDownload(_ download: DownloadFile) { ... NSWorkspace.shared.open(url) ... }
```
  (`DownloadManager.swift:342-350`). Verified by grep: zero `AVPlayer`, zero loopback HTTP server (`NWListener`/`GCDWebServer`) in the tree.
- Implication: playback requires the file on disk (or, in principle, an external app that tolerates a partially written file — aria2 preallocates with `file-allocation=prealloc`, `Aria2Preferences.swift:47`).

### Codec/quality selection
Only for yt-dlp-driven video downloads, three presets (`PeerKit/Sources/Models/YTDLPInvocation.swift:50-60`):
```swift
case .best:      "bv*+ba/b"
case .mp4:       "bv*[ext=mp4]+ba[ext=m4a]/b[ext=mp4]/b"
case .audioOnly: "bestaudio[ext=m4a]/bestaudio/best"
```
Invoked with a machine-readable progress template (`--progress-template "download:peeri-progress:%(progress)j"`, `YTDLPInvocation.swift:34-47`) parsed by `PeerKit/Sources/Models/YTDLPInvocation` + `Peeri/Services/YTDLPClient.swift` (269 lines). No ffprobe-based track selection for torrent content, and no container/codec probing at all.

### Subtitles
**None.** Verified: no subtitle handling anywhere in the Swift sources. Nothing to reuse.

### Concrete lessons to take
- Sidecar daemon + typed RPC client is a legitimate desktop architecture (crash isolation, mature BT engine); the tight version is what Peeri does: one `@Observable` model owning all state, one client, one poll loop.
- Render config from typed settings and hand it to the daemon as a file (`toAria2ConfigString`), keeping the daemon stateless across app restarts; `--save-session` / `--input-file` / `--force-save` / `--save-session-interval=5` make restart survivable.
- `--stop-with-process=<pid>` is a two-line fix for orphaned daemons when the GUI is force-quit — worth mirroring for any child process Peregrine spawns.
- `bt-prioritize-piece=head=4M,tail=4M` (plus `stream-piece-selector`) shows the minimum viable "preview" knob set that a real sequential downloader must replace with explicit piece-level scheduling.
- Security-scoped bookmarks (`Peeri/Services/DownloadDirectoryAccess.swift`) for user-chosen download folders is the correct macOS sandbox pattern; on Tauri the equivalent is persisted fs-scope permissions.
- Peer presentation (`PeerNetworkView`, `PieceGridView`, `Bitfield.swift`) turns raw aria2 status into a legible transfer inspector; `Bitfield` is unit-tested in isolation (`PeerKit/Tests/ModelsTests/BitfieldTests.swift`).
- Tracker blocklists are a first-class, updatable artifact (`Services/TrackerBlocklistUpdater.swift`, `Models/TrackerBlocklists.swift`), and peer-IP→country lookup is a separate service with its own test script (`Services/PeerCountryLookup.swift`, `scripts/test-peer-countries.swift`).
- Persist the *seed record* separately from the download list, and mark in-flight yt-dlp downloads as `failed` on launch (`DownloadManager.swift:76-79`) — honest restart semantics.

### Concrete mistakes / limits to avoid
- **Licensing trap:** the MIT repo commits ~57 MB of third-party binaries with no third-party notices in `LICENSE`: `aria2c` (7,981,800 bytes; aria2 is **GPL-2.0** `verified: https://raw.githubusercontent.com/aria2/aria2/master/COPYING`), `ffmpeg`/`ffprobe` (6,322,040 / 6,112,920 bytes; FFmpeg is **LGPL-2.1+** by default, GPL if built with GPL components `verified: https://raw.githubusercontent.com/FFmpeg/FFmpeg/master/LICENSE.md`), `yt-dlp` (37,146,048 bytes; **Unlicense** `verified: https://raw.githubusercontent.com/yt-dlp/yt-dlp/master/LICENSE`). MIT covers only Peeri's own code. Shipping aria2 in a proprietary app means GPL source-offer obligations; ship it as a separate user-installed process, or replace it with an MIT/Apache-licensed Rust BT engine.
- No torrent search, no subtitles, and no sequential streaming: the app's own README has to walk back the "video" claim. Treat Peeri as a *transfer manager* precedent, not a streaming precedent.
- 1-second polling of full status for every download does not scale to many active transfers and gives a laggy UI; prefer an event stream (aria2 also supports WebSocket notifications).
- `rpc-secret` is the hardcoded string `"peeri"` (`DownloadManager.swift:69`) and the daemon is launched with `rpc-listen-all=false` — fine on loopback, but any local process can drive it; per-launch random tokens are nearly free.
- One `@MainActor` class owns RPC, persistence, statistics, video downloads and settings application (664 lines) — same god-object risk as seerr's subscriber.
- `--stop-with-process` plus `terminationHandler` restart logic handles crashes, but there is no bounded restart/backoff policy documented; a crash loop would spin the daemon.

---

## 4. torlink (baairon/torlink)

### Purpose / stack
Terminal torrent finder + downloader + seeder. MIT `verified: https://github.com/baairon/torlink/blob/main/LICENSE`. 5,853 stars, 378 forks, repo size 278 KB, created 2026-06-25, `pushed_at` 2026-10-07. **Language is TypeScript, not Go** as the brief assumed: 123 `.ts` + 25 `.tsx` files, `engines.node >= 22`, deps `webtorrent ^2.4.1`, `parse-torrent`, `create-torrent`, `uint8-util`, `env-paths`, Ink 7 + React 19.

### Architecture
- `src/sources/*` — 10 pluggable search backends behind one `Source` interface (`sources/types.ts`), registered in `sources/registry.ts` (fitgirl, yts, tpbMovies, x1337Movies, eztv, tpbTv, x1337Tv, nyaa, subsplease, bittorrented).
- `src/download/{engine,queue,persist,history,playlist,reconcile,seed-reaper}` — `TorrentEngine` wraps one `WebTorrent` client (`engine.ts:38-72`), `DownloadQueue` (851 lines) owns items/seeds/history and a 500 ms poll (`queue.ts:51`).
- `src/daemon/*` — headless modes: `serve.ts` (HTTP add/control API on 9161), `files.ts` (read-only range-serving file server on 9160), `watch.ts` (watch folder), `runtime.ts`, `seed.ts`, `seed-reaper.ts`, `restart.ts`, `bootguard.ts`, `auth.ts`.
- `src/ui/*` — Ink TUI with a custom store; `src/config/*` — JSON config under `env-paths` dirs.
- Data flow: query → all sources concurrently → dedupe/sort → select result → magnet (from source or built from info hash) → `queue.add` → `TorrentEngine.add(magnet, {path, announce})` → poll stats → persist → on `done`, optionally seed and write `playlist.m3u`.

### Torrent search
- Fan-out is in one hook, with per-source status and a **150 ms coalesced flush** of partial results (`src/ui/hooks/useConcurrentSearch.ts:45-89`), plus `AbortController` cancellation on new query. Per-source errors are surfaced as `HTTP 404` / `no response` rather than aborting the search (`:16-19,106-114`).
- 5-minute in-process LRU of 100 entries keyed `sourceId::lowercased(query)` (`src/sources/cache.ts:3-41`); the comment records the actual bug: unbounded map held ~80 MB after 200 searches.
- Scrapers are honest about their limits: 1337x parses the table then fetches at most 4 detail pages (`MAX_DETAILS`, `src/sources/x1337.ts:10`) and rotates through four mirror hosts on failure (`HOSTS` `:6-7`, loop `:97-111`), re-filtering rows by meaningful query tokens (`:113-121`):
```ts
const raw = html.match(/magnet:\?xt=urn:btih:[^"'<>\s]+/i)?.[0];
if (!raw) return null;
return { magnet: unescapeEntities(raw), added: parseUploadDate(html) };
```
(`src/sources/x1337.ts:75-79` — magnet extraction from the detail page.) SubsPlease returns `seeders: 0` because its API has none (`sources/subsplease.ts:75`).

### Magnet resolution
- `buildMagnet` wraps a bare 40-hex/32-base32 info hash with 11 default trackers, mixing `udp://` and `http(s)://` "so peer discovery still works where UDP is blocked" (`src/sources/magnet.ts:1-30`); `base32ToHex` normalizes base32 hashes (`:36-56`).
- Sibling sources' trackers are folded onto the winning magnet without rebuilding it, preserving `xl`/`ws`/`so` params:
```ts
// `primary` is returned byte for byte with only the trackers it is missing appended
if (extra.length === 0) return primary;
return primary.trim() + extra.map((t) => `&tr=${encodeURIComponent(t)}`).join("");
```
(`src/sources/magnet.ts:113-134`.) A parsed `.torrent` is converted to a magnet for the queue (`magnetFromTorrentBytes`, `src/sources/torrentFile.ts`, max 16 MB).

### Sequential / streaming download
**Not implemented.** `TorrentEngine.add` passes only `{ path, announce }` (`src/download/engine.ts:95`):
```ts
const opts = announce && announce.length > 0 ? { path: dir, announce } : { path: dir };
...
torrent = client.add(source, opts);
```
The repo's own WebTorrent type shim exposes no file selection or priority API (`src/webtorrent.d.ts:32-35` — `TorrentOptions { path?, announce? }`), and `grep -rn "sequential|select\(|priorit"` over `src/` returns no download-ordering code. So pieces arrive in WebTorrent's default (rarest-first) order.

### Buffering / readahead
- **None during download.** Streaming is a post-download feature: `src/daemon/files.ts` serves the downloads directory over HTTP with single-range support — `parseRange` handles `bytes=a-b`, suffix ranges and 416 (`files.ts:76-98`), responds `206` + `Content-Range` + `Accept-Ranges: bytes`, and uses `pipeline()` rather than `pipe()` "so the read stream is destroyed when the client disconnects mid-transfer; a media player's seeks abort constantly, and plain pipe would leak an open handle per aborted request" (`files.ts:141-164`).
- `safeResolve` blocks traversal (`..`, absolute paths, encoded segments) and enforces the root prefix (`files.ts:52-66`). MIME map covers `.mkv .mp4 .webm .avi .mov .m3u .srt .vtt` etc. (`files.ts:19-43`).
- Consequence: with rarest-first piece order, an in-progress file served this way has holes and will not decode; torlink is a downloader with a file server, not a streamer.

### Codec/quality selection
**None.** No ffprobe/ffmpeg dependency; the only quality signals are size/seeders sorting (`src/ui/sort.ts`) and query filters (`src/ui/filter.ts`). Torrents are multi-file and unlabeled beyond name/size/seeders.

### Subtitles
**None.** `.srt`/`.vtt` appear only as MIME types in the file server (`files.ts:38-39`) and as playlist neighbours in a test fixture (`download/playlist.test.ts:19`). No fetching, parsing, timing, or rendering. What it *does* have is media-folder hygiene: `buildPlaylists` generates `playlist.m3u` per folder (natural sort, refuses to overwrite a file shipped by the torrent, `./`-prefixed relative entries so `#` names are not comments, `src/download/playlist.ts:19-52`).

### Concrete lessons to take
- A `Source` interface + registry makes "add a provider" a one-file change; per-source `SourceState {loading, error, code, count}` in the UI is what makes a multi-provider search debuggable (`useConcurrentSearch.ts:9-14`).
- Coalesce streamed partial results (150 ms) instead of re-rendering per source — directly applicable to a React front-end fed by Tauri events.
- Fold magnet trackers additively and byte-preservingly (`mergeMagnetTrackers`) rather than rebuilding the magnet; keep the source's own announce list authoritative and public trackers as fallback.
- Normalize info hashes once (base32→hex) and anchor the hash regex to the full string so a search query is never mistaken for a hash (`sources/magnet.ts:81-85`).
- Persist `.torrent` metadata after the `metadata` event so a later re-seed verifies on disk instead of re-fetching metadata from the swarm (`engine.ts:17-23`, `onMetadata` at `:105-112`).
- Guard every stats read: WebTorrent getters throw before metadata parses, and a throw inside the poll interval kills the process — one `try` around the whole stat block, partial numbers preferred (`engine.ts:155-170`).
- Crash-loop safety: a boot marker/`bootguard` that restores the queue in safe (paused) mode after a previous boot died mid-restore (`queue.ts:72-77`) — cheap and very effective for a desktop app.
- Headless API design worth imitating: `/health` unauthenticated, everything else token-gated, refuse to bind non-loopback without a token, and reject non-loopback `Host` headers when tokenless to defeat DNS rebinding (`daemon/serve.ts:337-363`, `daemon/files.ts:179-213`).
- Never let a network caller name a local path: `/add` accepts *bytes* (base64 `.torrent`) rather than a filesystem path, with a bencode sniff `bytes[0] !== 0x64` and a 1 MB cap (`serve.ts:22-26,48-79`).
- `./`-relative `#EXTM3U` playlists written only into directories that already exist and never over torrent-supplied files (`playlist.ts:36-52`) — a small, high-value media-library behaviour.
- Grace-period detectors beat naive ones: the "stray download means the seed's files are missing" check starts its timer when the on-disk piece check finishes (`onReady`), not at add time, because a 24 GB verify outran drain windows (`queue.ts:36-49`).

### Concrete mistakes / limits to avoid
- WebTorrent/`utp-native` operational fragility is baked into the source: on macOS NAT-PMP must be disabled (mDNSResponder owns UDP 5350) and uTP can exhaust sockets, requiring the `TORLINK_NO_UTP` opt-out (`engine.ts:44-67`). If Peregrine builds on WebTorrent, budget for these or use a Rust engine.
- No sequential/priority download and no readahead: torrents are unusable until complete, which is the single most important thing Peregrine must do differently.
- In-memory-only search cache (lost on restart) and a 1-second-ish poll for stats; fine for a TUI, insufficient for a desktop UI expecting instant feedback.
- Concurrency control is an env var only (`TORLINK_MAX_DOWNLOADS`, `queue.ts:56-59`), with 0 = unlimited as the default.
- History is capped at 500 in memory (`HISTORY_MAX`, `queue.ts:52`) and there is no searchable persistent index of past downloads beyond the JSON state files.
- The file server is rooted at the download directory and has no notion of "which torrent owns this file", so deleting a torrent does not invalidate served URLs (`daemon/files.ts`).

## 5. qBittorrent (qbittorrent/qbittorrent)

### Identity and provenance
GPL-2.0-or-later for the **source**; GPL-3.0-or-later for the **binary**, because bundled assets are GPLv3+; an OpenSSL linking exception applies to both. `verified: https://raw.githubusercontent.com/qbittorrent/qbittorrent/master/COPYING` (there is no `LICENSE`; the files are `COPYING`, `COPYING.GPLv2`, `COPYING.GPLv3`), first three lines verbatim:
```
The qBittorrent source code is licensed under the GNU General Public License, version 2 or (at your option) any later version (GPLv2+).
However, the binary distribution is licensed under GNU General Public License, version 3 or (at your option) any later version (GPLv3+),
because it contains GPLv3+ assets (eg images). In both cases, the following special exception is added:
```
Evidence URLs: `https://raw.githubusercontent.com/qbittorrent/qbittorrent/master/COPYING`, `.../COPYING.GPLv2`, `.../COPYING.GPLv3`. `api.github.com/repos/qbittorrent/qbittorrent/license` returned HTTP 403 (`"API rate limit exceeded for 41.90.177.253"`), so the SPDX id is my reading of COPYING's text, not an API value.

- Language/framework: C++, Qt 6 — `CMakeLists.txt:11` `set(minQt6Version 6.6.0)`; `cmake/Modules/CheckPackages.cmake:49` requires Qt6 `Core Network Sql Xml LinguistTools` (plus `CorePrivate` on Qt ≥ 6.10, DBus on Unix); libtorrent 1.2 or 2.x selected at build time via `QBT_USES_LIBTORRENT2`; Boost ≥ 1.76, OpenSSL ≥ 3.0.2.
- LOC (counted locally on the extracted tarball, `\n`+1, comments and blanks included): `src/**/*.{cpp,h,hpp}` = 609 files / 136,793 lines, of which **52,156 lines are vendored `src/base/3rdparty/`** (Lua 5.4 + LuaBridge). WebUI (`src/webui/www/`, excluding `translations/`) = 22,065 JS + 127 mjs + 13,333 HTML + 2,264 CSS. Search engine = 1,466 Python. Repo total 1,880 files.
- Snapshot: `master` = `8182ae071e8c49be178891940721debb5f373184`, authored 2026-10-07T01:34:34+08:00 ("Search: Show formal engine name in UI"), via `git ls-remote` + `git clone --filter=blob:none --depth 1`; tarball `https://codeload.github.com/qbittorrent/qbittorrent/tar.gz/refs/heads/master` (11,730,487 bytes). Master builds as **v5.4.0alpha1** (`src/base/version.h.in`: `QBT_VERSION_STATUS "alpha1"`).
- Latest stable release: **v5.2.4, 2026-09-28** (`https://github.com/qbittorrent/qbittorrent/releases.atom`, entry `updated 2026-09-28T19:02:55Z`; tag `release-5.2.4` = `7802fcc0a3eb`). Newest release of any kind is the prerelease **v5.3.0rc1** (2026-09-28T19:18:16Z, tag `6ed6f8d53170`). There is no `release-5.3.0` tag.
- Stars: **40,679** (`https://github.com/qbittorrent/qbittorrent` HTML, `"stargazerCount":40679`); the REST API was unusable for metadata/releases on every attempt.

### Architecture: the torrent session is UI-agnostic
- Four source trees with one-directional dependencies: `src/base/` (headless core, incl. `src/base/bittorrent/`), `src/gui/` (Qt Widgets), `src/webui/` (HTTP + JSON API), `src/app/` (composition root). `grep -rn "include \"gui/" src/base/` → **0 hits**; the only `gui/` includes are in `src/app/` (`application.cpp:99-103`, `main.cpp:80`). The boundary is one abstract interface + Qt signals: `session.h:138` declares `class Session : public QObject` with ~200 pure virtuals (`virtual bool isSequentialDownload() const = 0` style) and a signal set (`torrentsLoaded`, `torrentsUpdated`, `torrentAdded`, `torrentFinished`, `metadataDownloaded`, `statsUpdated`, `IPFilterParsed`). `SessionImpl` (`sessionimpl.cpp`, ~6.9k lines) is the only implementation, built in `src/app/application.cpp`; both front-ends reach it via `BitTorrent::Session::instance()` (108 call sites in `src/gui/`, plus `src/webui/api/*controller.cpp`).
- The GUI is a push-fed projection: `src/gui/transferlistmodel.cpp:133` connects `Session::torrentsUpdated` to `handleTorrentsUpdated` and mutates rows; `TransferListSortModel`/`TransferListWidget` sit above. The session posts `post_torrent_updates()`/`post_session_stats()` on a `refreshInterval()` timer (`SessionImpl::enqueueRefresh()`, `sessionimpl.cpp:6009`) and emits from the resulting libtorrent alerts; the GUI does not poll per torrent.
- `src/base/applicationcomponent.h` hands components a typed `IApplication *` (`src/base/interfaces/iapplication.h`); the GUI narrows it with `IGUIApplication` (`src/gui/interfaces/iguiapplication.h`) via the `GUIApplicationComponent<Base>` CRTP template. So `base/` components know `IApplication`, never `MainWindow`.
- Lesson: this is direct evidence for R6/R7/R8 — an engine namespace trait object plus an event stream, one adapter, and *two* independent front-ends added without touching `src/base/bittorrent/`. Peregrine's `TorrentEngine` trait + Tauri event channel is the same shape, and the arch check should assert the engine crate never depends on the UI crate.

### Play-while-downloading and sequential download (primary question)
**qBittorrent has no built-in streaming, no embedded player, no media/byte-range server, and nothing that reacts to a player's read position.** It is an external-player client with two independent, per-torrent piece-ordering switches.
- Sequential ordering is one libtorrent flag. `src/base/bittorrent/torrentimpl.cpp:1750`:
```cpp
void TorrentImpl::setSequentialDownload(const bool enable)
{
    if (enable)
    {
        m_nativeHandle.set_flags(lt::torrent_flags::sequential_download);
        m_nativeStatus.flags |= lt::torrent_flags::sequential_download;  // prevent return cached value
    }
    else
    {
        m_nativeHandle.unset_flags(lt::torrent_flags::sequential_download);
        m_nativeStatus.flags &= ~lt::torrent_flags::sequential_download;
    }
    deferredRequestResumeData();
}
```
  Also applied at add time from `AddTorrentParams.sequential` (`sessionimpl.cpp:3079-3082`); exposed as `-s/--sequential`, env `QBT_SEQUENTIAL` (`cmdoptions.cpp:327,441,525`), `Torrent::toggleSequentialDownload()` (`torrent.cpp:64`) and `/api/v2/torrents/toggleSequentialDownload` (`webui/api/torrentscontroller.cpp:1649`).
- "Download first and last pieces first" is **not** a libtorrent flag: qBittorrent builds a full piece-priority vector and calls `prioritize_pieces()`. `src/base/bittorrent/torrentimpl.cpp:1781` `applyFirstLastPiecePriority()`:
```cpp
const int numPieces = std::ceil(fileSize(fileIndex) * 0.01 / pieceLength());
for (int i = 0; i < numPieces; ++i)
{
    piecePriorities[pieceRange.first() + i] = piecePrio;
    piecePriorities[pieceRange.last() - i] = piecePrio;
}
const int firstPiece = pieceRange.first() + numPieces;
const int lastPiece = pieceRange.last() - numPieces;
for (int pieceIndex = firstPiece; pieceIndex <= lastPiece; ++pieceIndex)
    piecePriorities[pieceIndex] = LT::toNative(filePrio);
...
m_nativeHandle.prioritize_pieces(piecePriorities);
```
  The 1% window carries the comment "worst case: AVI index = 1% of total file size (at the end of the file)" — this is a header/index reachability optimisation, applied to *every* file in the torrent, not a readahead window for the file a player is about to open.
- Persistence: the sequential flag rides inside libtorrent's own resume data (`LoadTorrentParams::ltAddTorrentParams` is an `lt::add_torrent_params`, round-tripped by `lt::write_resume_data`/`lt::read_resume_data`); first/last-piece is a separate qBittorrent column (`has_outer_pieces_priority`, `DB_COLUMN_HAS_OUTER_PIECES_PRIORITY`) re-applied on startup, metadata receipt and file-priority change (`torrentimpl.cpp:405`, `:1950`, `:3062`).
- **The gap Peregrine targets, stated plainly:** nothing computes piece priorities from a playback position. `grep -rn "set_piece_deadline\|piece_deadline\|add_piece" src/` (excluding `3rdparty`) → **0 hits**; there is no deadline scheduling, no readahead window, and no reader feedback of any kind. The WebUI has no byte-range endpoint — the only `StreamFileAPIResult` (`src/webui/api/apicontroller.h:53`) is produced solely by `APIController::setResult(const Path&)` and consumed at `webapplication.cpp:382` to serve static WebUI assets. "Preview" requires the file to already exist and hands the OS a **path**:
```cpp
const Path path = m_torrent->actualStorageLocation() / m_torrent->actualFilePath(fileIndex);
if (!path.exists())
{ ... QMessageBox::critical(parent, tr("Preview impossible"), ...); return; }
emit readyToPreviewFile(path);
```
  (`src/gui/previewselectdialog.cpp`); `TransferListWidget::previewFile` is just `Utils::Gui::openPath(filePath);` (`transferlistwidget.cpp:245-247`). Playback begins only after the bytes are on disk.
- Transferable: keep sequential ordering and head/tail priority as two separate, persisted, per-torrent concepts. Rejectable: relying on `sequential_download` alone — a whole-torrent ordering hint with no deadline or QoS input cannot bound startup latency for one chosen file.

### Queue, limits, categories, tags, and the persisted model
- Queue positions are owned by **libtorrent's** auto-management and only mirrored by qBittorrent: `increaseTorrentsQueuePos`/`decrease…`/`top…`/`bottom…` (`sessionimpl.cpp:2756-2793`) call `torrentQueuePositionUp/Down/Top/Bottom` free functions on the native handle; `saveTorrentsQueue()` (`:3423`) builds a sparse `QList<TorrentID>` from `torrent->queuePosition()` and persists it through `ResumeDataStorage::storeQueue`, flagging `m_torrentsQueueChanged`/`m_needSaveTorrentsQueue`. Caps `maxActiveDownloads`/`maxActiveUploads`/`maxActiveTorrents` are clamped to `>= -1`, where −1 means unlimited.
  Auto-management is libtorrent's `setAutoManaged` plus qBittorrent policy (`handleQueueingModeChanged`, `ignoreSlowTorrentsForQueueing`, AutoTMM) whose side effects are explicit edge-triggered booleans (`DisableAutoTMMTriggers/CategoryChanged`, `…/CategorySavePathChanged`, `…/DefaultSavePathChanged`) rather than implicit coupling.
- Share limits are a **three-tier cascade** — per-torrent → category → global — resolved by sentinels, not by copying values. `src/base/bittorrent/sharelimits.h`: `DEFAULT_RATIO_LIMIT = -2` (inherit) / `NO_RATIO_LIMIT = -1` (unlimited), the same pair for seeding time and inactive seeding time; `TorrentImpl::effectiveShareLimits()` (`torrentimpl.cpp:1082-1095`) substitutes the category value wherever the torrent says `Default`. `ShareLimitAction { Stop=0, Remove=1, RemoveWithContent=3, EnableSuperSeeding=2 }` carries the comment "These values should remain unchanged when adding new items so as not to break the existing user settings."
- States: `TorrentState` in `src/base/bittorrent/torrent.h` — 19 values incl. `QueuedDownloading`/`QueuedUploading`, `CheckingResumeData`, `StalledDownloading`, `Moving`, `MissingFiles`, and a single `Error`. Completed is not a state; it is `hasFinishedStatus`/`completedTime` (`progress() == 1` plus `m_completedTime`).
- Persistence is a two-implementation abstraction, `ResumeDataStorage` (`resumedatastorage.h`, `load`/`store`/`remove`/`storeQueue`): SQLite `torrents.db` (`DBResumeDataStorage`) is the modern default, and `BT_backup/<40-hex>.fastresume` + `<id>.torrent` (`BencodeResumeDataStorage`, `bencoderesumedatastorage.cpp:123,152-153`) is both migration source and fallback — `prepareStartup()` (`sessionimpl.cpp:1461-1483`) selects primary/startup storage in *both* directions.
- SQLite schema (`src/base/bittorrent/dbresumedatastorage.cpp:100-160, 440-500`): table `torrents`, one row per torrent, columns `id`, `torrent_id BLOB UNIQUE`, `queue_position INTEGER NOT NULL DEFAULT -1`, `name`, `category`, `tags`, `comment`, `target_save_path`, `download_path`, `content_layout`, `ratio_limit`, `seeding_time_limit`, `inactive_seeding_time_limit`, `share_limits_mode`, `share_limit_action`, `has_outer_pieces_priority`, `has_seed_status`, `operating_mode`, `stopped`, `stop_condition`, `ssl_certificate/private_key/dh_params`, `libtorrent_resume_data BLOB NOT NULL`, `metadata BLOB`, plus an index on `queue_position`; table `meta(name TEXT UNIQUE, value BLOB)` holds the schema version.
- Categories are **not** in that DB: `CATEGORIES_FILE_NAME {u"categories.json"}` (`sessionimpl.cpp:126`) is written to `Config/categories.json` via `AsyncFileStorage` (`:5872-5910`) as a nested tree of `CategoryOptions { savePath, downloadPath, shareLimits }`, expanded into dotted names. The global tag union is the config key `BitTorrent/Tags`; per-torrent category/tags live in the resume row. Settings are one flat in-memory hash (`settingsstorage.cpp`) serialised to INI `qBittorrent.conf`.
- Adopt: the `meta` version table plus additive `updateDB(fromVersion)` migration (`dbresumedatastorage.cpp:508-575`) — `if (fromVersion <= N)` steps guarded by an "does this column exist?" probe, with `dbVersion` inferred as 1 when `download_path` is absent so even a pre-versioning DB migrates. That is exactly our append-only migration rule, already field-tested. Also adopt the sentinel cascade and the split of "small nested config tree → JSON file, per-item rows → relational table". Reject: `Default`/`None` **string** sentinels stored in the same column as real values; one torrent's state spread across four stores with no single writer; a single `Error` state with no structured error kind; queue positions owned by the engine's internal counter rather than by our own durable ordering.

### Search plugins: Python scripts, loaded without integrity checks or sandboxing
- `src/base/search/searchpluginmanager.cpp` downloads plugins into `engines/` from `https://raw.githubusercontent.com/qbittorrent/search-plugins/refs/heads/master/nova3/engines/` (`:93`). There is **no hash, no signature, no commit pinning** — the only validation is the `.py` extension (`installPlugin`, `:236-253`) plus a "newer version wins" check parsed out of the script itself. Updates overwrite the same path; `clearPythonCache()` deletes `__pycache__`/`*.pyc` first (`:61-84`) so stale bytecode cannot mask a swap.
  Execution shells out to a **system** Python found on `PATH` (`python3`, then `python`; `src/base/utils/foreignapps.cpp pythonInfo()`, overridable by a preference), running `src/searchengine/nova3/nova2.py` as a `QProcess` with `-I -Xutf8=1` (`PYTHON_ISOLATE_MODE_FLAG`, `PYTHON_UTF8_MODE_FLAG` in `src/base/utils/foreignapps.h:40-41`), a 3-minute kill timer, and `CloseFileDescriptors` on Unix (`searchhandler.cpp:98-129`). `-I` only strips `PYTHONPATH`/user-site/cwd from `sys.path` and ignores `PYTHON*` env vars; it is **not a sandbox**.
- **Plugins can execute arbitrary code.** `src/searchengine/nova3/nova2.py:130-144`:
```python
def import_engine(engine_module_name: EngineModuleName) -> Optional[type[Engine]]:
    ...
    try:
        engine_module = importlib.import_module(f"engines.{engine_module_name}")
        engine_class = getattr(engine_module, engine_module_name)
```
  `import_module` runs module top-level code on import, with the user's full privileges: filesystem, network, subprocess, credentials. Plugins also get a SOCKS shim (`socks.py`) and inherit whatever qBittorrent passes in the process environment. (qBittorrent separately embeds a *Lua* plugin engine for its own extensions — `src/base/plugins/pluginsengine.cpp`, `plugin.cpp` with a `QDeadlineTimer` watchdog at `LUA_TIMEOUT` — which is also in-process; the search path is the looser of the two.)
- This is the concrete evidence for R25: a remote-code-execution channel that (a) fetches code from a mutable third-party branch over HTTPS with no integrity check, (b) is triggered by an ordinary user action ("check for updates") and installs *new* engines, and (c) runs in the same OS security context as the component that parses magnets, bencode and untrusted torrent metadata. Compromise of a plugin or of the raw.githubusercontent path yields code execution on every install. Compiled, in-repo, allowlisted Rust providers behind one provider registry remove the class entirely. **Reference only — do not port, and do not adopt a script host at all.**

### Network safety: IP filter, anonymous mode, proxy, interface binding
- IP filter: `src/base/bittorrent/filterparserthread.cpp` picks a parser purely by file extension in `run()` — PeerGuardian `.p2p`, PeerGuardian `.p2b` (`parseP2BFilterFile`), eMule `.dat` (`parseDATFilterFile`) — into one `lt::ip_filter`. Application is deliberately atomic; `enableIPFilter()` (`sessionimpl.cpp:5965-5990`) records the reason: "Set the ip_filter in one go so there isn't a time window where there isn't an ip_filter set between clearing the old one and setting the new one." Runtime bans and file rules are merged by `processBannedIPs` and re-applied even when filtering is disabled, so switching the list off does not silently unban manual bans.
  **No hot reload, and failure is fail-open.** `grep -rn QFileSystemWatcher src/` matches only `src/gui/mainwindow.cpp:534` (executable watcher) and `src/base/torrentfileswatcher.cpp` (watch folders) — nothing watches the filter file, so an in-place overwrite of `ipfilter.dat` is picked up only when a setting changes (`m_IPFilteringConfigured = false`). There is no `.gz` support. A parse error calls `handleIPFilterError()`, which installs an **empty** `lt::ip_filter` plus a warning and emits `IPFilterParsed(true, 0)` — defensible for a blocklist, but it must be a deliberate choice rather than an inherited one.
- Anonymous mode is exactly one libtorrent setting: `settingsPack.set_bool(lt::settings_pack::anonymous_mode, isAnonymousModeEnabled())` (`sessionimpl.cpp:2159`), persisted as `BitTorrent/AnonymousModeEnabled`. It is libtorrent's user-agent/peer-id scrubbing, not routing — worth naming honestly in our UI.
- Proxy: `configure()` (`sessionimpl.cpp:2045-2084`) maps `Net::ProxyType` → `socks4 | socks5 | socks5_pw | http | http_pw`, sets hostname/port/credentials, and adds two independent booleans: `proxy_peer_connections` (qBittorrent's "proxy peer connections", `BitTorrent/ProxyPeerConnections`) and `proxy_hostnames` (`proxyConfig.hostnameLookupEnabled`). There is **no separate UDP/DHT proxy switch** and qBittorrent does not disable DHT when SOCKS5 is chosen, so "am I actually tunnelled?" depends on libtorrent's `proxy_type` semantics. Our design must expose UDP/DHT coverage explicitly instead of inheriting this ambiguity.
- Interface binding — **fail-closed, deliberately.** `getListeningIPs()` (`sessionimpl.cpp:3520-3600`) resolves the configured interface/address; if it cannot be found it logs `Log::CRITICAL` and returns the unresolved string rather than falling back to all interfaces, with the reason in the source:
```cpp
// Pass the invalid user configured interface name/address to libtorrent
// in hopes that it will come online later.
// This will not cause IP leak but allow user to reconnect the interface
// and re-establish connection without restarting the client.
IPs.append(ifaceAddr);
```
  `applyNetworkInterfacesSettings()` (`:2300-2367`) then guarantees `outgoing_interfaces` is never empty for the same stated reason: "Otherwise an empty string will be passed to outgoing_interface which will cause IP leak." On Windows an unresolvable GUID falls back to the interface *name*, not to `0.0.0.0`. When the user pinned a port, `max_retry_port_bind` is set to `0` so binding fails instead of silently succeeding on another port (`:2305-2306`). The cost: nothing re-resolves interfaces at runtime — `m_listenInterfaceConfigured` is cleared only by the setters (`:3620`) and there is no `QNetworkInformation` usage — so when a VPN interface disappears, libtorrent keeps retrying the dead binding. That is the leak-free behaviour we want, **plus** a required addition: surface "interface gone" as a user-visible state and stop the transfer instead of grinding. Reference only.

### Operational hardening
- Atomic writes by construction: `Utils::IO::saveToFile` (`src/base/utils/io.cpp:131-152`) uses `QSaveFile` — temp file, `write`, `flush`, `commit` (rename) — for both raw bytes and bencode. Config loss avoidance: `SettingsStorage::writeNativeSettings()` (`src/base/settingsstorage.cpp:165-215`) writes `qBittorrent_new.conf`, calls `sync()`, checks `status()`, and only then removes+renames over the real file, recording why: "QSettings deletes the file before writing it out. This can result in problems if the disk is full or a power outage occurs." On load (`:118-160`) a **non-empty `_new` file is preferred** and logged as "Detected unclean program exit. Using fallback file to restore settings". A read-only config file returns "no need to retry saving" instead of error-looping.
- Config migration: `src/app/upgrade.cpp` (735 lines) is a single monotonic `Meta/MigrationVersion` integer with `if (version < N) migrateX();` steps (`:644-690`), per-key renames (`migrate(oldKey, newKey, savePath)`), and a deserialiser that copies the **entire** INI hash into memory first so keys inserted by hand or disabled by `#ifdef` survive a rewrite (`settingsstorage.cpp:125-140`). Same spirit as the resume-DB version table; both are adoptable as-is.
- Corrupt data: a failed `load()` for one torrent logs `Log::CRITICAL` and **skips that torrent only** (`processNextResumeData`, `sessionimpl.cpp:1561-1568`) — one bad row never blocks startup. Bencode of the resume blob and embedded metadata is bounded by tunable limits: `BitTorrent/BdecodeDepthLimit` default 100, `BitTorrent/BdecodeTokenLimit` default 10,000,000, `BitTorrent/TorrentFileSizeLimit` default 100 MiB (`src/base/preferences.cpp:323-360`) — a bencode-bomb guard checked before decoding untrusted bytes.
- Single instance and IPC: `src/app/localpeer.cpp` uses a `QLockFile` (stale-lock time `0`, explicit `removeStaleLockFile()` when the recorded `hostid`/`hostname` match this machine) plus a `QLocalServer` at `<config>/ipc-socket` with `QLocalServer::UserAccessOption`; messages over `MAX_MESSAGE_SIZE` are dropped ("drop suspiciously large data") and the client retries twice "in case the other instance is just starting up". A second launch serialises command-line params (`@sequential`, `@firstLastPiecePriority`, `@savePath`, `@category`, …) and the first instance replays them via `Application::processMessage` (`src/app/application.cpp:554`), queueing them until startup completes.
- Update check: `src/gui/programupdater.cpp` fetches only `https://www.qbittorrent.org/versions.json` (fallback `qbittorrent.github.io/qBittorrent-website/versions.json`) then `QDesktopServices::openUrl(.../download)` — no auto-download, no silent install. Untrusted metadata is treated as an injection source: `Application::runExternalProgram` deliberately does not forward the user's shell environment because doing so "could enable command injection via torrent name and other arguments (especially when some automated download mechanism has been setup)" (issue #10925) — the right posture for any "run program on completion" feature.
- Licensing trap in one file: `src/app/localpeer.cpp` / `.h` are **not** GPL — their header is the Qt Solutions BSD notice (`$QT_BEGIN_LICENSE:BSD$`, "Copyright (C) 2013 Digia Plc and/or its subsidiaries"). It is the only file I found whose header would not contaminate a permissive codebase, but it is still third-party code with attribution terms, and we do not need it.

### WebUI API v2 — the network contract, its sentinels, and its version drift
Read from the 5.0 wiki cross-checked against tag source, because the wiki is stale in three places.
- Shape `/api/v2/<scope>/<method>`, GET or POST only (wrong method 405 since 4.4.4); every method needs auth except `auth/login` ([General Information](https://github.com/qbittorrent/qBittorrent/wiki/WebUI-API-(qBittorrent-5.0)#general-information)). WebAPI version maps per release: 4.6.7→2.9.3, 5.0.0→2.11.2, 5.1.0→2.11.4, 5.2.0→2.15.1, master→2.17.0 (`src/webui/webapplication.h`).
- **Auth**: cookie session from `POST /api/v2/auth/login` (`Set-Cookie`), with `Referer`/`Origin` required to match host:port and a banned IP answering 403. **Cookie name is `SID` through 5.1 and `QBT_SID_<port>` from 5.2** — a client must read it from `Set-Cookie` rather than hardcode `SID` ([webapplication.cpp](https://github.com/qbittorrent/qBittorrent/blob/master/src/webui/webapplication.cpp#L689-L842)). An API key (`Authorization: Bearer` or `X-API-Key`, 32 chars, `qbt_` prefix, one at a time) exists from 5.2. Failed login answers **HTTP 200 with body `Fails.` through 5.1 and 401 from 5.2** — a silent difference a health check can misread.
- **Add** (`POST /torrents/add`, multipart): `urls` (newline-separated magnet/http), `torrents` (raw bytes, repeatable; invalid file → 415), `savepath`, `category`, `tags`, `skip_checking`, `root_folder`, `rename`, `upLimit`, `dlLimit`, `ratioLimit`, `seedingTimeLimit`, `autoTMM`, `sequentialDownload`, `firstLastPiecePrio` ([Add new torrent](https://github.com/qbittorrent/qBittorrent/wiki/WebUI-API-(qBittorrent-5.0)#add-new-torrent)). **The wiki's `paused` is stale:** 4.6.7 reads `paused`, 5.0.0 reads `stopped`; 5.0 also adds `addToTopOfQueue`, `downloadPath`/`useDownloadPath`, `stopCondition`, `contentLayout`, `inactiveSeedingTimeLimit`, `shareLimitAction`.
- **Inspect**: `/torrents/info` — `state, progress (0..1), eta, dlspeed/upspeed, size, total_size, completed, amount_left, save_path, content_path, category, tags, ratio, seeding_time, completion_on, seq_dl, f_l_piece_prio`; `/torrents/files` — `index, name, size, progress, priority (0 skip / 1 normal / 6 high / 7 maximal), is_seed, piece_range`; `/torrents/properties`; `/torrents/pieceStates` (0/1/2).
- **The state machine is precedence-ordered in the engine, not the wiki**: `checkingResumeData → moving → missingFiles → error → no metadata → checking → finished → otherwise`, where finished resolves to the seed set and 5.0+ emits `stoppedUP`/`stoppedDL` (not `pausedUP`/`pausedDL`) plus `forcedMetaDL` ([torrentimpl.cpp](https://github.com/qbittorrent/qBittorrent/blob/master/src/base/bittorrent/torrentimpl.cpp#L1289-L1347)).
- **Actions**: `/torrents/stop` + `/torrents/start` (5.0; `pauseAction`/`resumeAction` in 4.6.7), `delete` (`deleteFiles`), `recheck`, `reannounce`, `setLocation`, `setCategory`, `addTags`/`removeTags` (`setTags` only from 5.1), `filePrio`, and the queue verbs. **`/torrents/status` does not exist** in the official API — a design that names it is wrong.
- **Three sentinels that produce embarrassing UI if read naively**: `completed` is a **byte count, not a boolean**; `eta` uses **8640000 seconds to mean unknown/infinite**; `ratio` returns **-1** for undefined/infinite. Completion is `progress == 1.0`, equivalently `amount_left == 0`, equivalently membership in the seed state set.
- **Metadata gates the file list**: `/torrents/files` is empty while metadata is absent, so a player — or our safety gate — must wait for the metadata phase rather than read an empty list as "no files".
- **For Peregrine:** the engine port owns all of this in-process, so the cookie/CSRF/Origin/API-key/static-asset/queue-delta surface is not ours to build. What we *do* inherit is the semantics: the precedence-ordered state machine, the metadata gate before files are exposed, the `stopped`/`stoppedUP` naming, and the three sentinels. Those belong in our engine adapter's state mapping, not in the UI.

### Licensing boundary
- **GPL-2.0-or-later (source) / GPL-3.0-or-later (binary). No qBittorrent file may be copied, adapted, or paraphrased into Peregrine.** Every `src/**` file read carries the GPLv2-or-later header block plus the OpenSSL linking exception; the exception permits linking OpenSSL only and does not relax copyleft. Read for headers: `src/base/bittorrent/{session.h,torrent.h,torrentimpl.cpp,sessionimpl.cpp,addtorrentparams.h,dbresumedatastorage.cpp,bencoderesumedatastorage.cpp,filterparserthread.cpp,searchpluginmanager.cpp}`, `src/base/settingsstorage.cpp`, `src/gui/programupdater.cpp`, `src/app/upgrade.cpp`, `COPYING`.
- Per-lesson classification:
  - **Reference only — reimplement from described behaviour:** the `Session`/`Torrent` interface + signal design; push-based model/view updates; the sentinel share-limit cascade; the head/tail priority maths; the SQLite resume schema and its additive migration; the `_new`-then-rename config write with unclean-exit recovery; the `Meta/MigrationVersion` migration; fail-closed interface resolution; atomic IP-filter swap; the lock-file + local-socket single-instance protocol; the 1%/head-tail heuristic.
  - **Safe to reimplement (facts and standard technique, not expression):** piece-priority ordering strategies; bencode depth/token/size limits; `QSaveFile`-style write-temp-then-rename; format detection by extension; sentinels for "inherit"; a lock-file + retry handshake; fetching a versions manifest and opening a download page rather than self-updating.
  - **Reject outright:** the Python search-plugin loader (unauthenticated remote code execution in-process), and any dependency on a system Python.
- Contamination flag: any Peregrine file derived from qBittorrent source is reached by GPL-2.0-or-later (§5(a) of `COPYING.GPLv2`) and by the distribution's GPLv3+ asset obligation. The sole permissive header found is the Qt Solutions BSD block on `src/app/localpeer.{cpp,h}`; treat it as third-party and still do not copy.

### Concrete lessons to take
1. One abstract `Session` + one implementation + two front-ends (Qt and WebUI) is proof that an engine/GUI split survives a second UI; enforce it with an arch check that the engine crate cannot depend on the UI crate.
2. Fail-closed binding: never substitute all-interfaces when the pinned interface or address is missing — pass the unresolved value to the engine, log critically, let the bind fail; and set `max_retry_port_bind = 0` when the user pinned a port so a silent port change is impossible.
3. Atomic-by-construction state: write-temp-then-rename for every write, prefer a leftover `_new` file on load after an unclean exit, use monotonic integer schema versions (a `meta` table for the torrent store, one preferences key for config), migrate with additive `ALTER TABLE ADD COLUMN` steps, and isolate failures per item so one corrupt record is skipped rather than fatal.
4. Bound every parser before decoding untrusted bytes: explicit bencode depth and token ceilings plus a torrent-file size cap (100/10M/100 MiB here) — the same discipline applies to our `.torrent`, magnet and subtitle parsers.
5. Model "inherit" explicitly: a three-tier per-torrent → category → global cascade with distinct sentinels for *inherit* and *unlimited* is far cleaner than copying resolved values down, and it survives a later change of the global default.

### Concrete mistakes / limits to avoid
- Only a whole-torrent ordering hint (`sequential_download`) plus a static head/tail window: no deadline scheduling, no readahead window, no reader feedback (`set_piece_deadline` appears nowhere in `src/`). This is the gap Peregrine's scheduler must close, and it is the strongest justification for building our own piece scheduler rather than toggling an engine flag.
- No `.gz`/`.zip` IP-filter support and no watcher-driven reload; a parse error fails open to an empty blocklist. Anonymous mode and proxy coverage each look like one "safe" switch while UDP/DHT tunnelling is left to engine defaults, so the UI does not say what is actually proxied.
- `Q_ENUM_NS` enums rely on a comment ("these values should remain unchanged") and store `Default`/`None` string sentinels alongside real values — express that invariant as `#[repr(i32)]` plus a migration test, and carry a structured error kind instead of collapsing every failure into one `Error` state.
- Per-torrent state lives in four stores (INI, `categories.json`, `torrents.db`, `BT_backup/`) and two resume storages can both be live during migration; one authoritative store, one writer. `sessionimpl.cpp` (~6.9k lines) and `torrentimpl.cpp` (~3.1k lines) are god objects spanning settings, queueing, alerts, IP filtering, proxies and file management — split by concern. `Changelog` says "Unreleased … v5.3.0" while `version.h.in` says 5.4.0alpha1: generate the version string instead of hand-maintaining two.

### Verification status
- **Verified by reading source:** every file path, line reference and excerpt above; the absence of `set_piece_deadline`/`add_piece`/deadline scheduling and of any media-streaming or byte-range serving path; zero `gui/` includes from `src/base/`; the Python plugin import mechanism and the absence of hash/signature/pinning; the fail-closed interface-resolution comments; `QSaveFile` and `_new`-file atomicity; both migration mechanisms; the `localpeer` BSD header; LOC counts and the 52,156/136,793 vendored-3rdparty split.
- **Verified by raw fetch / non-API URLs:** `COPYING`, `COPYING.GPLv2`, `COPYING.GPLv3`; `releases.atom` (v5.2.4 = 2026-09-28, v5.3.0rc1 = 2026-09-28, no v5.3.0); star count from the repo page HTML; tag SHAs and commit dates via `git ls-remote` and a shallow clone.
- **Could not verify, and why:**
  - `api.github.com` was rate-limited for this IP on every attempt (HTTP 403, `"API rate limit exceeded for 41.90.177.253"`), including `/license`, `/releases/latest` and the repo metadata endpoint. The SPDX id `GPL-2.0-or-later` is my reading of `COPYING`'s first sentence (source) and `GPL-3.0-or-later` of its second (binary); GitHub's own classification was not retrievable.
  - The snapshot is `master` (tarball from `codeload`), not a release artifact. Claims about released binaries — specifically which bundled assets force the GPLv3+ binary licence — come from `COPYING`'s text only; those assets were not audited.
  - Whether shipped builds use libtorrent 1.2 or 2.x changes a few paths behind `#ifdef QBT_USES_LIBTORRENT2`; the source was read as-is and never built.
  - The Lua plugin engine (`src/base/plugins/`) was inventoried by paths and headers only. I saw a `QDeadlineTimer` timeout (`plugin.cpp:58-66`) but did not audit whether any capability restriction exists.
  - The WebUI's JavaScript/TypeScript runtime was not read in depth; only the API controller endpoints and `webapplication.cpp` result dispatch were inspected.
  - Whether any fork or downstream package adds streaming was not investigated; only upstream `master` was read.
  - The legal reading of GPL/OpenSSL-exception interaction is a summary of the licence text, not legal advice.

---

---

## 6. Prowlarr (Prowlarr/Prowlarr)

### Identity and role
- GPL-3.0-or-later (declared in `sidecars.toml`, ADR 0024; licence text not re-verified here). Snapshot: `develop` at `b8d04898207b16070c010c8d74b4cc218ff84b8f`; wiki <https://wiki.servarr.com/prowlarr>.
- Role: **indexer manager and aggregator**. It owns indexer definitions, credentials, categories, health/backoff and one search API. It has no library, no quality profile, no release decision, and no download client of its own — its optional download clients exist only to serve its own `POST /search` grab.

### Indexer definitions are data (Cardigann YAML)
- One YAML file per indexer: `id/name/description/language/type/encoding`, `requestDelay`, `followredirect`, `testlinktorrent`, `links/legacylinks`, `certificates`, `caps`, `settings`, `login`, `search`, `download` ([CardigannDefinition.cs](https://github.com/Prowlarr/Prowlarr/blob/b8d04898207b16070c010c8d74b4cc218ff84b8f/src/NzbDrone.Core/Indexers/Definitions/Cardigann/CardigannDefinition.cs#L30-L210), [wiki](https://wiki.servarr.com/prowlarr/cardigann-yml-definition)).
- `caps`: `categorymappings` (tracker category → Newznab cat) and `modes` (`search:q` mandatory; tv/movie/music/book-search). `type` is public/semi-private/private.
- `settings` are the UI fields the user fills (default username/password when omitted); `search` gives `paths`/`rows`/`fields` against HTML, JSON or XML; `download` exists only when the `.torrent` is not on the search page, needs POST, or a pre-request.
- Built-ins ship from `indexers.prowlarr.com/master/12` (list + `package.zip`) into AppData `Definitions/`; custom `*.yml` live in `Definitions/Custom/`.

### The search API
- `GET /api/v1/search` — `query`, `type` (default `search`), `indexerIds`, `categories`, `limit`, `offset`; none hard-required, empty `indexerIds` means every enabled indexer, and `-1`/\`-2\` are the Usenet/torrent pseudo-ids.
- `ReleaseResource`: `guid, indexerId, indexer, title, size, publishDate, downloadUrl, magnetUrl, infoHash, infoUrl, categories, protocol, seeders, leechers, files, ...` ([ReleaseResource.cs](https://github.com/Prowlarr/Prowlarr/blob/b8d04898207b16070c010c8d74b4cc218ff84b8f/src/Prowlarr.Api.V1/Search/ReleaseResource.cs#L14-L57)).
- Categories are Newznab numeric ids 1000-8000 plus `100000 Custom`, as a tree.

### The download proxy: 301 to a magnet, or 200 with the .torrent
**This is the behaviour Peregrine's acquire job hit and had to fix.**
- `/api/v1/search` returns **no raw URLs**: both `downloadUrl` and `magnetUrl` are rewritten to
  `{serverUrl}/{indexerId}/download?apikey=KEY&link=<AES-encrypted base64url>&file=<title>`
  where `link` is encrypted with Prowlarr's persisted `DownloadProtectionKey` (tamper and open-proxy defence) and `file` is display only ([SearchController.cs](https://github.com/Prowlarr/Prowlarr/blob/b8d04898207b16070c010c8d74b4cc218ff84b8f/src/Prowlarr.Api.V1/Search/SearchController.cs#L189-L200), [DownloadMappingService.cs](https://github.com/Prowlarr/Prowlarr/blob/b8d04898207b16070c010c8d74b4cc218ff84b8f/src/NzbDrone.Core/Download/DownloadMappingService.cs#L30-L41)). A null field stays null.
- `GET /{indexerId}/download` decrypts `link`, then enforces enabled (**410**), backing-off (**429** + `Retry-After`) and over-grab-limit (**429**) before resolving ([NewznabController.cs](https://github.com/Prowlarr/Prowlarr/blob/b8d04898207b16070c010c8d74b4cc218ff84b8f/src/Prowlarr.Api.V1/Indexers/NewznabController.cs#L209-L263)):
  - normal Cardigann torrent (`SupportsRedirect => false`): a `magnet:` link is validated and returned as bytes **with no network call**, and the controller answers **301 `Location: magnet:...`**; an http `.torrent` is fetched (redirects followed, a magnet hop handled) and answered **200 `application/x-bittorrent`** ([NewznabController.cs](https://github.com/Prowlarr/Prowlarr/blob/b8d04898207b16070c010c8d74b4cc218ff84b8f/src/Prowlarr.Api.V1/Indexers/NewznabController.cs#L296-L314), [HttpIndexerBase.cs](https://github.com/Prowlarr/Prowlarr/blob/b8d04898207b16070c010c8d74b4cc218ff84b8f/src/NzbDrone.Core/Indexers/HttpIndexerBase.cs#L227-L260)).
  - Usenet or a redirect-capable indexer: a plain **301** to the decrypted URL.
- So this URL is a **"resolve to torrent-file-or-magnet" primitive**, and a client must follow the redirect. An HTTP client that cannot follow a non-HTTP scheme sees only a bare 301 — exactly the failure that left our acquire job at 500 permille.
- Sonarr/Radarr consume it as a synced Torznab indexer (`baseUrl={ProwlarrUrl}/{indexerId}/`, `apiPath=/api`), storing the proxy link as the download URL and following the 301 at grab time.

### App sync is dead weight without Sonarr/Radarr
- `SyncLevel` is `Disabled | AddOnly | FullSync`; tags choose which indexers an app receives; **Sync Profiles** (`AppSyncProfile`: per-indexer `EnableRss/EnableAutomaticSearch/EnableInteractiveSearch/MinimumSeeders` via `AppProfileId`) are a different mechanism from tags ([ApplicationService.cs](https://github.com/Prowlarr/Prowlarr/blob/b8d04898207b16070c010c8d74b4cc218ff84b8f/src/NzbDrone.Core/Applications/ApplicationService.cs#L161-L234), [AppSyncProfile.cs](https://github.com/Prowlarr/Prowlarr/blob/b8d04898207b16070c010c8d74b4cc218ff84b8f/src/NzbDrone.Core/Profiles/AppSyncProfile.cs)).
- All of it exists to push indexers over Torznab/Newznab into *arr. With no *arr, bypass `ApplicationService`/`AppIndexerMap`/`SyncLevel`/`/api/v1/applications`, the Torznab XML, and `POST /api/v1/search` (which needs a Prowlarr download client and a 30-minute cache hit).

### Health, backoff and limits — the model to copy (R24)
- `IndexerStatus` = `EscalationLevel`, `InitialFailure`, `MostRecentFailure`, `DisabledTill`, cookies; disabled ⇔ `DisabledTill > now` ([IndexerStatus.cs](https://github.com/Prowlarr/Prowlarr/blob/b8d04898207b16070c010c8d74b4cc218ff84b8f/src/NzbDrone.Core/Indexers/IndexerStatus.cs)).
- Backoff periods `[0, 60, 300, 900, 1800, 3600, 3h, 6h, 12h, 24h]`; a **429 uses `Retry-After` (1 h floor) and jumps at least to that level**; a **connection failure does not escalate**; a success decrements one level; a 15-minute startup grace caps it ([ProviderStatusServiceBase.cs](https://github.com/Prowlarr/Prowlarr/blob/b8d04898207b16070c010c8d74b4cc218ff84b8f/src/NzbDrone.Core/ThingiProvider/Status/ProviderStatusServiceBase.cs#L53-L150), [EscalationBackOff.cs](https://github.com/Prowlarr/Prowlarr/blob/b8d04898207b16070c010c8d74b4cc218ff84b8f/src/NzbDrone.Core/ThingiProvider/Status/EscalationBackOff.cs)).
- Per-definition `requestDelay`; per-indexer query/grab limits counted over an hour/day window, with `Retry-After` computed from the oldest in-window event ([IndexerLimitService.cs](https://github.com/Prowlarr/Prowlarr/blob/b8d04898207b16070c010c8d74b4cc218ff84b8f/src/NzbDrone.Core/Indexers/IndexerLimitService.cs)).
- A blocked indexer drops out of the enabled set, so it silently stops appearing in searches.

### Auth
- One API key in `config.xml`, accepted as `X-Api-Key`, `?apikey=`, or `Authorization: Bearer`. The download proxy embeds it in the query string because clients fetch a URL without headers.

### Concrete lessons to take
1. Indexer health is **persisted state with an escalating, 429-aware backoff** — that table is the design `PG-E12-01`'s rate declarations should match, and it is the R24 work currently owed.
2. `{id}/download` is a **resolver, not a file path**: one URL legitimately answers 200 bytes or 301 magnet. Model it as one typed outcome (our `FetchedTorrent`), never as "a magnet".
3. Definitions are **data, not code** (YAML + a selector engine), which is how one sidecar covers hundreds of trackers. If Peregrine ever bundles definitions, pin the set and never self-update from a remote — the contrast with qBittorrent's unsigned Python plugin loader is instructive.

### Concrete mistakes / limits to avoid
- The search API hides the raw magnet behind an encrypted proxy hop, so any client that assumes `magnetUrl` is a magnet is wrong by construction.
- Do not port the app-sync surface "for later": with no *arr it has no user and no behaviour (R3).

### Verification status
- **Verified by reading source at `b8d04898`:** every file path, line range and behaviour above, including the proxy rewrite, both download branches, the backoff table, the limit service and the app-sync gating.
- **Not verified:** which HTTP stack inside *arr or a download client performs the 301 follow (Sonarr only stores the URL); the licence text (taken from our own `sidecars.toml`/ADR 0024).

---

## 7. Sonarr and Radarr (Sonarr/Sonarr, Radarr/Radarr)

### Identity and role
- **The brain.** They own the library, the release decision, the download-client configuration, the queue, the import, and history/blocklist. They own **no** indexer of their own when Prowlarr/Jackett feeds them (see §6) and no transfer engine (see §5). Licence not re-verified here.

### What actually triggers a search
- **Neither app routinely searches the back catalogue.** Both poll indexers for newly posted releases and diff against wanted/missing/cutoff-unmet items: Sonarr RSS every 10-120 min, Radarr every 15-60 min ([Sonarr FAQ](https://wiki.servarr.com/sonarr/faq#how-does-sonarr-find-episodes), [Radarr FAQ](https://wiki.servarr.com/radarr/faq#how-does-radarr-work)).
- Active (API) searches happen only for a user/API search, "Add and Search", Wanted → Missing / Cutoff Unmet bulk, and "Recent Episodes".
- Each indexer toggles **Enable RSS / Enable Automatic Search / Enable Interactive Search** independently.
- Every release carries its origin in code — `Rss`, `ReleasePush`, `Search`, `UserInvokedSearch`, `InteractiveSearch` ([DownloadDecisionMaker.cs](https://github.com/Sonarr/Sonarr/blob/develop/src/NzbDrone.Core/DecisionEngine/DownloadDecisionMaker.cs)).
- An external manager can push into `POST /api/v3/release/push`, which runs the **same decision engine synchronously** ([ReleasePushController.cs](https://github.com/Sonarr/Sonarr/blob/develop/src/Sonarr.Api.V3/Indexers/ReleasePushController.cs)).

### The decision engine — the part worth adapting
- Per release: parse the title → map to series/movie + episodes → compute custom formats → score → run the specification list ([DownloadDecisionMaker.cs](https://github.com/Sonarr/Sonarr/blob/develop/src/NzbDrone.Core/DecisionEngine/DownloadDecisionMaker.cs)).
- **The comparer is an ordered key list, not one opaque score** — Sonarr: Quality → Custom Format score → Protocol → Episode Count → Episode Number → Indexer Priority → Seeders/Peers → Age → Size; Radarr: Quality → CF score → Protocol → Indexer Priority → Indexer Flags → Seeds/Peers → Age → Size. REPACK/PROPER rank above the same quality ([Sonarr FAQ](https://wiki.servarr.com/sonarr/faq#how-are-possible-downloads-compared), [DownloadDecisionComparer.cs](https://github.com/Sonarr/Sonarr/blob/develop/src/NzbDrone.Core/DecisionEngine/DownloadDecisionComparer.cs)).
- **Quality profile** fields: `Name`, `UpgradeAllowed`, `Cutoff`, `MinFormatScore`, `CutoffFormatScore`, `MinUpgradeFormatScore`, `FormatItems`, `Items` ([QualityProfile.cs](https://github.com/Sonarr/Sonarr/blob/develop/src/NzbDrone.Core/Profiles/Qualities/QualityProfile.cs)).
- **Quality definitions** carry `Weight`, `MinSize`/`MaxSize`/`PreferredSize` in MB/min ([QualityDefinition.cs](https://github.com/Sonarr/Sonarr/blob/develop/src/NzbDrone.Core/Qualities/QualityDefinition.cs)); `AcceptableSizeSpecification` **scales the bound by total runtime**, skips unknown-size releases and specials, and falls back to series runtime (45 min for a pilot). A separate global maximum caps everything ([AcceptableSizeSpecification.cs](https://github.com/Sonarr/Sonarr/blob/develop/src/NzbDrone.Core/DecisionEngine/Specifications/AcceptableSizeSpecification.cs)).
- **Custom formats** are conditions — Release Title (regex, matched against the release title *and* the post-download filename), Language, Source, Resolution, Size, Release Group, Indexer Flag, Release Type (Radarr adds Edition). Different condition types **AND**; multiple conditions of one type **OR** unless *Required*. Matches are computed on the fly and not persisted; the score is the sum of the matching formats' profile scores, and **`MinFormatScore` is a hard grab gate** ([CustomFormatCalculationService.cs](https://github.com/Sonarr/Sonarr/blob/develop/src/NzbDrone.Core/CustomFormats/CustomFormatCalculationService.cs), [CustomFormatAllowedByProfileSpecification.cs](https://github.com/Sonarr/Sonarr/blob/develop/src/NzbDrone.Core/DecisionEngine/Specifications/CustomFormatAllowedByProfileSpecification.cs)).
- **Upgrades** are one function with a documented check order: `UpgradableSpecification.IsUpgradable` returns `BetterQuality | BetterRevision | QualityCutoff | CustomFormatScore | CustomFormatCutoff | MinCustomFormatScore | UpgradesNotAllowed` ([UpgradableSpecification.cs](https://github.com/Sonarr/Sonarr/blob/develop/src/NzbDrone.Core/DecisionEngine/Specifications/UpgradableSpecification.cs)).
- **Minimum availability** (Radarr): `Announced | InCinemas | Released`, where Released is `min(Digital, Physical)`, falling back to `InCinemas + 90 days`, shifted by the Availability Delay setting ([Movie.cs](https://github.com/Radarr/Radarr/blob/develop/src/NzbDrone.Core/Movies/Movie.cs)).
- **Rejections are first-class.** Each spec returns `Accept()` or `Reject(reason, message)` with a `Priority` and a **Permanent/Temporary** type; the engine evaluates specs grouped by priority ascending and **stops at the first group that rejects**, so cheap checks short-circuit disk/database ones; a thrown error becomes a `DecisionError` rejection ([DownloadSpecDecision.cs](https://github.com/Sonarr/Sonarr/blob/develop/src/NzbDrone.Core/DecisionEngine/DownloadSpecDecision.cs), [SpecificationPriority.cs](https://github.com/Sonarr/Sonarr/blob/develop/src/NzbDrone.Core/DecisionEngine/SpecificationPriority.cs)). `DownloadRejectionReason` is a ~80-value enum (`QualityNotWanted`, `CustomFormatMinimumScore`, `Blocklisted`, `BelowMinimumSize`, `UpgradesNotAllowed`, `MinimumSeeders`, ...) ([DownloadRejectionReason.cs](https://github.com/Sonarr/Sonarr/blob/develop/src/NzbDrone.Core/DecisionEngine/DownloadRejectionReason.cs)).
- Surfaced as `ReleaseResource { Approved, TemporarilyRejected, Rejected, Rejections[], CustomFormatScore, QualityWeight, ReleaseWeight }`, and the UI draws the reason per row ([ReleaseResource.cs](https://github.com/Sonarr/Sonarr/blob/develop/src/Sonarr.Api.V3/Indexers/ReleaseResource.cs)).

### The download-client abstraction — mostly server scaffolding
- Port `IDownloadClient` (`Protocol`, `Download(...)`, `GetItems`, `GetFiles`, `GetImportItem`, `RemoveItem`, `GetStatus`, `MarkItemAsImported`); `DownloadClientBase<TSettings>` adds HTTP 5xx/timeout retry and remote path mapping; `TorrentClientBase` has `AddFromMagnetLink`/`AddFromTorrentFile` ([IDownloadClient.cs](https://github.com/Sonarr/Sonarr/blob/develop/src/NzbDrone.Core/Download/IDownloadClient.cs), [TorrentClientBase.cs](https://github.com/Sonarr/Sonarr/blob/develop/src/NzbDrone.Core/Download/TorrentClientBase.cs)).
- Generic definition: `Protocol`, `Priority` (1-50, round-robin within priority), `RemoveCompletedDownloads` (default true), `RemoveFailedDownloads` (default true) ([DownloadClientDefinition.cs](https://github.com/Sonarr/Sonarr/blob/develop/src/NzbDrone.Core/Download/DownloadClientDefinition.cs)).
- The qBittorrent adapter's settings are the interesting part: `TvCategory` (default `tv-sonarr`), `TvImportedCategory`, `RecentTvPriority`/`OlderTvPriority`, `InitialState`, `SequentialOrder`, `FirstAndLast`, `ContentLayout`, `AddSeriesTags`, plus host/credentials ([QBittorrentSettings.cs](https://github.com/Sonarr/Sonarr/blob/develop/src/NzbDrone.Core/Download/Clients/QBittorrent/QBittorrentSettings.cs)).
- Hand-off: pick clients by protocol + indexer + series/movie tags (or an explicit override), **try each in order and fall back on failure**, rate-limit grabs (~2/s), then publish a *grabbed* event carrying the client and download id ([DownloadService.cs](https://github.com/Sonarr/Sonarr/blob/develop/src/NzbDrone.Core/Download/DownloadService.cs)).

### Queue, import, history, blocklist
- `TrackedDownload` states: `Downloading → ImportBlocked/ImportPending → Importing → Imported`, plus `FailedPending`, `Failed`, `Ignored`; the per-download snapshot is `DownloadClientItem` (progress, sizes, ratio, output path, encrypted, removable) ([TrackedDownload.cs](https://github.com/Sonarr/Sonarr/blob/develop/src/NzbDrone.Core/Download/TrackedDownloads/TrackedDownload.cs), [DownloadClientItem.cs](https://github.com/Sonarr/Sonarr/blob/develop/src/NzbDrone.Core/Download/DownloadClientItem.cs)).
- The app **polls the client** and reconciles (Radarr's finished-download interval is min 1 minute) — an embedded engine can emit events instead.
- Import resolves the target by parsing the download title and **falls back to the series id recorded at grab time**, else it is `ImportBlocked` ([CompletedDownloadService.cs](https://github.com/Sonarr/Sonarr/blob/develop/src/NzbDrone.Core/Download/CompletedDownloadService.cs)).
- Transfer: Usenet moves atomically; torrents **hardlink-or-copy so seeding continues** ([TransferMode.cs](https://github.com/Sonarr/Sonarr/blob/develop/src/NzbDrone.Common/Disk/TransferMode.cs)).
- **History is an append-only event log** (`Grabbed`, `DownloadFolderImported`, `DownloadFailed`, ...), each row carrying the download id and a key/value data bag (client, size, indexer, release group, match type, release source) ([EpisodeHistory.cs](https://github.com/Sonarr/Sonarr/blob/develop/src/NzbDrone.Core/History/EpisodeHistory.cs)).
- **Blocklist rows are keyed by info hash** and the decision engine rejects any future match of one via `BlocklistSpecification` (`Blocklisted`) ([Blocklist.cs](https://github.com/Sonarr/Sonarr/blob/develop/src/NzbDrone.Core/Blocklisting/Blocklist.cs)).
- Failed handling: a failure moves to `FailedPending`, publishes `DownloadFailedEvent`, then optionally removes from the client, redownloads, and blocklists. **Both wikis say this is Usenet-only; Radarr states it does not apply to torrents** ([FailedDownloadService.cs](https://github.com/Sonarr/Sonarr/blob/develop/src/NzbDrone.Core/Download/FailedDownloadService.cs)).
- Queue removal: `DELETE /api/v3/queue/{id}?removeFromClient=&blocklist=&skipRedownload=&changeCategory=` ([QueueController.cs](https://github.com/Sonarr/Sonarr/blob/develop/src/Sonarr.Api.V3/Queue/QueueController.cs)).

### The API surface a caller uses
- Trigger: `POST /api/v3/command {name:"MissingEpisodeSearch", seriesId}` / `{name:"MoviesSearch", movieIds:[id]}`.
- Add: `POST /api/v3/series` / `POST|PUT /api/v3/movie`, lookup `GET /series/lookup?term=tvdb:ID` / `GET /movie/lookup?term=tmdb:ID`.
- Monitor: `GET /api/v3/queue?includeEpisode=true` → `status, trackedDownloadStatus, trackedDownloadState, downloadId, protocol, downloadClient, indexer, size, sizeleft, timeleft`.
- Reference: `GET /system/status`, `/qualityProfile`, `/tag`, `POST /tag`, `/rootFolder`.
- Manual: `GET /api/v3/release?...` (returns `ReleaseResource` with rejections) and `POST /api/v3/release` to grab a cached result, with `ShouldOverride` to force.
- Seerr itself was observed using only `/command`, `/queue`, and add/lookup (`/release` was not seen in its servarr clients).

### Concrete lessons to take
1. **Specification list + typed rejection reasons**: small pure predicates, each accepting or rejecting with a reason, a priority order, and permanent-vs-temporary semantics. Ours is the domain's ranking with per-rule reasons — keep the shape and surface it (`PG-E12-10`).
2. **A deterministic ordered comparer**, not a single opaque score, so ties and upgrades are explainable.
3. **Size limits scaled by runtime**, with explicit unknown-size and unknown-runtime branches and a global maximum — an edge case we must not skip.
4. **Custom-format scoring as a stateless matcher** (AND across condition types, OR within, `Required` overrides) with a hard minimum-score gate.
5. **Tracked-download state machine** (`Downloading → ImportPending/ImportBlocked → Importing → Imported` + `Failed`/`Ignored`), because our current job states are thinner than the real lifecycle.
6. **History as an append-only event log** and a **blocklist keyed by info hash** that the decision engine consults automatically — this is the retry loop we are missing.
7. **Hardlink-or-copy import with seeding left in place.**

### Concrete mistakes / limits to avoid
- The download-client abstraction, multi-client failover, remote path mapping, client-specific seed-goal capability tables and push ingestion all exist because Sonarr babysits an external process; an embedded engine collapses them to internal settings.
- Polling the client to infer completion is a workaround for not owning the engine.
- **Do not copy Usenet-only failed-download handling onto torrents** — Radarr says explicitly it does not apply, and treating a stalled torrent as a failed Usenet download would blocklist healthy releases.

### Verification status
- **Verified by reading source + wiki:** the comparer orders, quality-profile fields, size specifications, custom-format matching and scoring, the upgrade check order, the specification priority/stop-at-first-rejecting-group behaviour, the rejection enum and `ReleaseResource`, the download-client port/settings, the tracked-download states, import fallback, transfer modes, history/blocklist shapes, and the queue API.
- **Not verified:** the SPDX licence of Sonarr/Radarr (not read here); whether any Seerr code path other than the servarr clients uses `/release`.

## Cross-cutting recommendations for Peregrine
1. `peregrine-providers` should be a Rust trait returning our own normalized types (not a borrowed TMDB shape), with a cache-aside layer and per-provider key ceilings — seerr's structure without seerr's type leak.
2. `peregrine-torrent` needs what none of Peeri, torlink, or qBittorrent has: explicit piece-level scheduling (sequential plus head/tail priority), deadline-driven readahead tied to the demuxer's next-needed byte range, and a byte-range HTTP server that can answer for *in-flight* data. qBittorrent returns zero hits for `set_piece_deadline`/`add_piece` across its source, so this is a genuine gap in the state of the art rather than a refinement. Adopt VLC's `stream_t` semantics as the interface.
3. Prefer spawning a sidecar for anything GPL (aria2) or LGPL-linked (libvlc); keep the Rust core MIT/Apache so the licensing surface stays clean.
4. Copy the operational hygiene, which is where these codebases are strongest: bootguard safe mode, seed reaper with a duration grammar, watchdog/restart around child processes, token+Host checks on any loopback server, bencode/size validation on uploads, range parsing with 416 handling, `pipeline`-style cancellation on client disconnect.
5. Ship notification and subtitle pipelines as trait + registry designs from day one (seerr's agents; VLC's `spu decoder` capability), because both are additive features that rot into god objects otherwise.
6. **The four systems draw the same four seams — and the seams are not essential.** Prowlarr (indexer aggregation), Sonarr/Radarr (decision + lifecycle), qBittorrent (transfer), Jellyseerr (request/discovery UX) are separate processes because each has a different owner and lifetime, not because the responsibilities cannot live together. Peregrine keeps the four as **ports** (ADR 0008) and deletes the HTTP between them: `Indexer` (`PG-E12-01/02`), the domain decision engine (`PG-E4-05/06`, `PG-E12-07/08`), `AcquisitionService` (`PG-E10-19`) over `TorrentEngine` (librqbit), and the title/discover surface with `TitleOffer`. The architecture is already the all-in-one version of the reference stack; what is missing is inside two of the boxes.
7. **Adopt the specification-list + typed-rejection shape as the decision model.** Sonarr/Radarr evaluate small pure predicates, each accept/reject with a typed reason, grouped by priority, **stopping at the first rejecting group**, with permanent-vs-temporary semantics and a stable reason enum surfaced on every result. Our ranking already returns typed per-rule reasons; make that the durable model and persist it for `PG-E12-10`'s "why this release" panel. This is the single highest-value import.
8. **Add the two loops we lack.** (a) A **blocklist keyed by info hash** that the decision engine consults automatically, so a failed release is never re-grabbed but can be forced; (b) an **upgrade policy** — cutoff plus a minimum increment — so a better release replaces a worse one (`PG-E12-09`). Without them a bad grab is permanent and a good upgrade never happens.
9. **Model the download lifecycle properly.** Our `download_job` states are thinner than the real one: `Downloading → ImportPending/ImportBlocked → Importing → Imported`, plus `Failed`/`Ignored`. Import must resolve its target by parsing the release title and **falling back to the target recorded at grab time**, and must prefer **hardlink-or-copy so seeding continues** (`PG-E14-19`).
10. **Size limits are runtime-scaled**, with explicit unknown-size and unknown-runtime branches and a separate global maximum. That edge case belongs in the quality-definition model (`PG-E4-05/06`), not in a later patch.
11. **Availability is a reconciliation job, not only a read.** Jellyseerr recomputes from the library *and* the acquisition queue on a timer and flips content to `DELETED` when it disappears; keeping a derived read for the UI is fine at our scale, but the reconciliation pass is what stops the projection drifting.
12. **Request granularity is the season.** Jellyseerr's request body has `seasons: number[] | 'all'` and **no episode field at all** — each season carries its own state, and episode detail exists only in availability. When the owed season/episode/multi-selection work lands, make the request record season-level and keep episodes in the availability projection; do not put per-episode state on the request.
13. **Indexer health is persisted state with an escalating, 429-aware backoff** (§6). That is the concrete design for the per-host rate limiting R24 currently owes (`PG-E12-01`).
14. **Do not port the seams themselves.** Prowlarr's app-sync, `/applications`, SyncLevel/AppSyncProfile and Torznab XML; Sonarr's download-client abstraction, multi-client failover, remote path mapping and Usenet-only failed-download handling; Jellyseerr's quota/approval layer unless we add multi-user. Each is scaffolding for a separate process (R3).
15. **The pre-write safety gate is ours alone.** None of the four inspects a torrent before writing; Sonarr hands bytes to a client and imports afterwards. Peregrine's gate runs *before* the engine is handed the torrent (FLOWS F6), which is exactly why acquisition is a job with a persisted report rather than an immediate add. Nothing in the reference architecture weakens that, and no lesson above should be allowed to move it after the add.

## Verification status
- Verified by reading source: all file paths, excerpts, enums, entity/column lists, LOC counts, license header classifications, Peeri's lack of search/streaming/subtitles, torlink's lack of sequential download/subtitles, VLC's removal of the libvlc event API.
- Verified by API/raw fetch: repo metadata (stars, size, `pushed_at`, default branch), license SPDX + license file SHAs, last-5 commit lists, aria2/FFmpeg/yt-dlp/WebTorrent license texts.
- **Not verified / uncertain:**
  - "Last-commit activity" reflects GitHub's snapshot at 2026-10-08T22:5x UTC; `pushed_at` was 2026-10-07 for VLC while the newest commit shown was 2026-10-03, so the mirror may lag or have non-default-branch pushes.
  - The brief described torlink as Go; the repository is TypeScript. I could not find any Go code.
  - VLC's LOC excludes `contrib/` and `po/`; a full-tree count would be larger, and generation/third-party code inside `contrib/` was not counted.
  - Peeri's bundled `ffmpeg`/`ffprobe` build flags (and therefore whether they are LGPL-2.1-only or GPL) could not be determined from the binary or repo; only FFmpeg's default licensing was verified.
  - The legal reading of LGPL-2.1 dynamic linking for libvlc is a summary of the license text and VLC's README, not legal advice; no lawyer-reviewed source was consulted.
  - Peeri has no published architecture docs; `CLAUDE.md` and `SETUP.md` were not mined beyond the README.
  - torlink's search sources were read for 1337x/subsplease/magnet/registry; the other seven backends were inventoried by path and interface only, not read line by line.
