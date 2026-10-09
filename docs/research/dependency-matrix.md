# Peregrine dependency matrix

Verified 2026-10-08 against crates.io API v1, registry.npmjs.org and raw upstream sources with
`User-Agent: peregrine-plan`. Nothing is guessed; unresolved items are in section 6.
**version** = latest stable (a newer prerelease is shown in parentheses where it changes the decision).
**last release** = date of that stable version. **signal** = crates.io 90-day downloads.
Maintained = a release since 2025-10-08.

## 1. Rust dependencies

| crate | version | license | last release | signal (90d dl) | verdict |
|---|---|---|---|---|---|
| tauri | 2.12.1 (newest 3.0.0-alpha.4) | Apache-2.0 OR MIT | 2026-09-30 | 13.7M | keep |
| tauri-build | 2.7.1 (newest 3.0.0-alpha.3) | Apache-2.0 OR MIT | 2026-09-30 | 13.7M | keep |
| tauri-plugin-updater | 2.13.2 | Apache-2.0 OR MIT | 2026-10-07 | 5.4M | keep |
| tauri-plugin-store | 2.5.0 (newest 3.0.0-alpha.2) | Apache-2.0 OR MIT | 2026-09-26 | 1.5M | keep |
| tauri-plugin-window-state | 2.5.0 (newest 3.0.0-alpha.2) | Apache-2.0 OR MIT | 2026-09-26 | 2.0M | keep |
| tauri-plugin-single-instance | 2.5.2 | Apache-2.0 OR MIT | 2026-10-01 | 4.5M | keep |
| tauri-plugin-os | 2.4.0 (newest 3.0.0-alpha.2) | Apache-2.0 OR MIT | 2026-09-26 | 1.4M | keep |
| tauri-plugin-notification | 2.5.1 | Apache-2.0 OR MIT | 2026-10-01 | 3.2M | keep |
| tauri-plugin-deep-link | 2.6.1 | Apache-2.0 OR MIT | 2026-10-01 | 2.8M | keep |
| tauri-plugin-shell | 2.4.1 | Apache-2.0 OR MIT | 2026-10-07 | 4.0M | keep |
| tauri-plugin-opener | 2.7.0 (newest 3.0.0-alpha.2) | Apache-2.0 OR MIT | 2026-09-29 | 5.7M | keep |
| tauri-plugin-dialog | 2.8.1 (newest 3.0.0-alpha.3) | Apache-2.0 OR MIT | 2026-10-01 | 7.8M | keep |
| tauri-plugin-fs | 2.6.0 (newest 3.0.0-alpha.2) | Apache-2.0 OR MIT | 2026-09-26 | 8.1M | keep |
| tauri-plugin-http | 2.8.1 | Apache-2.0 OR MIT | 2026-10-07 | 1.3M | keep |
| tauri-plugin-log | 2.10.0 (newest 3.0.0-alpha.2) | Apache-2.0 OR MIT | 2026-09-26 | 2.4M | keep |
| tauri-plugin-global-shortcut | 2.4.0 (newest 3.0.0-alpha.2) | Apache-2.0 OR MIT | 2026-09-26 | 1.9M | keep |
| tauri-plugin-clipboard-manager | 2.4.1 | Apache-2.0 OR MIT | 2026-10-01 | 2.3M | keep |
| tauri-plugin-process | 2.4.0 (newest 3.0.0-alpha.2) | Apache-2.0 OR MIT | 2026-09-26 | 3.7M | keep |
| tauri-plugin-autostart | 2.7.0 | Apache-2.0 OR MIT | 2026-10-01 | 2.1M | keep |
| specta | 1.0.5 (newest 2.0.0-rc.25) | MIT | 2023-07-17 | 1.3M | investigate - no stable 2.x; only 2.0.0-rc.25. Pin exact, expect RC churn |
| tauri-specta | 1.0.2 (newest 2.0.0-rc.25) | MIT | 2023-05-18 | 624k | investigate - no stable 2.x; only 2.0.0-rc.25. Pin exact |
| tokio | 1.53.2 | MIT | 2026-10-03 | 251.1M | keep |
| tokio-util | 0.7.19 | MIT | 2026-07-21 | 187.0M | keep |
| futures | 0.3.34 | MIT OR Apache-2.0 | 2026-08-11 | 187.8M | keep |
| serde | 1.0.229 | MIT OR Apache-2.0 | 2026-07-18 | 349.8M | keep |
| serde_json | 1.0.151 | MIT OR Apache-2.0 | 2026-07-20 | 350.0M | keep |
| thiserror | 2.0.21 | MIT OR Apache-2.0 | 2026-09-23 | 412.0M | keep |
| anyhow | 1.0.104 | MIT OR Apache-2.0 | 2026-07-18 | 236.2M | keep |
| tracing | 0.1.44 | MIT | 2025-12-18 | 214.4M | keep |
| tracing-subscriber | 0.3.23 | MIT | 2026-03-13 | 157.1M | keep |
| tracing-appender | 0.2.5 | MIT | 2026-04-17 | 36.2M | keep |
| sqlx | 0.9.0 | MIT OR Apache-2.0 | 2026-05-21 | 44.0M | keep |
| rusqlite | 0.40.2 | MIT | 2026-08-08 | 41.1M | keep, but redundant - sqlx already covers SQLite. Pick one |
| refinery | 0.10.0 | MIT | 2026-10-02 | 2.2M | keep |
| librqbit | 9.0.1 | Apache-2.0 | 2026-08-20 | 60k | keep |
| axum | 0.8.9 | MIT | 2026-04-14 | 129.7M | keep |
| hyper | 1.12.0 | MIT | 2026-10-06 | 227.1M | keep |
| tower | 0.5.3 | MIT | 2026-01-12 | 191.7M | keep |
| tower-http | 0.7.1 | MIT | 2026-08-31 | 169.3M | keep |
| reqwest | 0.13.5 | MIT OR Apache-2.0 | 2026-09-08 | 209.0M | keep |
| reqwest-middleware | 0.5.2 | MIT OR Apache-2.0 | 2026-05-19 | 19.6M | keep |
| reqwest-retry | 0.9.1 | MIT OR Apache-2.0 | 2026-02-05 | 9.9M | keep |
| http-cache | 0.21.0 (newest 1.0.0-alpha.8) | MIT OR Apache-2.0 | 2025-06-26 | 1.0M | investigate - current line but 15mo since release |
| http-cache-reqwest | 0.16.0 (newest 1.0.0-alpha.9) | MIT OR Apache-2.0 | 2025-06-26 | 944k | investigate - same repo/train as http-cache |
| governor | 0.10.4 | MIT | 2025-12-16 | 18.1M | keep |
| backoff | 0.4.0 | MIT/Apache-2.0 | 2021-12-14 | 16.0M | replace with backon - abandoned since 2021; librqbit already uses backon |
| moka | 0.12.16 | (MIT OR Apache-2.0) AND Apache-2.0 | 2026-08-09 | 40.4M | keep |
| dashmap | 6.2.1 | MIT | 2026-05-17 | 89.8M | keep |
| ulid | 3.0.0 | MIT | 2026-07-16 | 15.6M | keep |
| jiff | 0.2.38 | Unlicense OR MIT | 2026-10-06 | 80.0M | keep |
| time | 0.3.55 | MIT OR Apache-2.0 | 2026-08-01 | 196.9M | keep |
| chrono | 0.4.45 | MIT OR Apache-2.0 | 2026-06-04 | 194.6M | keep, but jiff + time already cover it. Drop to cut a dep tree |
| chardetng | 1.0.0 | Apache-2.0 OR MIT | 2026-03-30 | 5.6M | keep |
| encoding_rs | 0.8.42 | (Apache-2.0 OR MIT) AND BSD-3-Clause | 2026-09-24 | 119.6M | keep |
| image | 0.25.10 | MIT OR Apache-2.0 | 2026-03-10 | 58.3M | keep |
| fast_image_resize | 6.1.0 | MIT OR Apache-2.0 | 2026-07-21 | 5.0M | keep |
| blurhash | 0.2.3 | Apache-2.0/MIT | 2024-07-23 | 469k | keep |
| lofty | 0.25.4 | MIT OR Apache-2.0 | 2026-09-20 | 452k | keep |
| walkdir | 2.5.0 | Unlicense/MIT | 2024-03-01 | 162.1M | keep |
| jwalk | 0.9.0 | MIT | 2026-08-05 | 2.5M | keep |
| notify | 8.2.0 (newest 9.0.0-rc.5) | CC0-1.0 | 2025-08-03 | 42.9M | keep |
| fs4 | 1.1.0 | MIT OR Apache-2.0 | 2026-04-28 | 19.1M | keep |
| sysinfo | 0.39.6 | MIT | 2026-07-09 | 52.1M | keep |
| keyring | 4.2.0 | MIT OR Apache-2.0 | 2026-08-29 | 13.5M | keep |
| secrecy | 0.10.3 | Apache-2.0 OR MIT | 2024-10-09 | 45.8M | keep |
| zeroize | 1.9.1 | Apache-2.0 OR MIT | 2026-10-06 | 198.5M | keep |
| mdns-sd | 0.21.5 | Apache-2.0 OR MIT | 2026-10-05 | 2.7M | keep |
| rust_cast | 0.21.0 | MIT | 2025-12-30 | 26k | investigate |
| ssdp | 0.7.0 | MIT/Apache-2.0 | 2018-10-05 | 710 | replace with rupnp - last release 2018, dead |
| dlna | - | - | - | - | unverified (not found on crates.io) |
| libmpv2 | 6.0.0 | LGPL-2.1 | 2026-05-12 | 40k | investigate - crate is LGPL-2.1 and Rust links statically; 66 stars |
| mpv-client | 1.1.0 | GPL-3.0 | 2025-06-28 | 324 | drop - GPL-3.0, unmaintained; GPL contamination |
| raw-window-handle | 0.6.2 | MIT OR Apache-2.0 OR Zlib | 2024-05-17 | 32.3M | keep |
| ffmpeg-sidecar | 2.6.0 | MIT | 2026-09-30 | 576k | keep |
| subparse | 0.7.0 | MPL-2.0 | 2021-04-11 | 4k | keep |
| ass-parser | 0.2.3 | MIT OR Apache-2.0 | 2025-04-11 | 151 | investigate - 6 stars, 13k lifetime downloads |
| srt | 0.1.0 | MIT | 2017-03-10 | 464 | replace - 2017 stub, 5.7k lifetime downloads. Use subparse or own parser |
| srt-rs | 0.2.3 | MPL-2.0 | 2020-12-13 | 82 | drop - this is libsrt (SRT transport) bindings, not subtitles |
| opener | 0.9.0 | MIT OR Apache-2.0 | 2026-09-29 | 6.3M | keep, but tauri-plugin-opener already covers this |
| directories | 6.0.0 | MIT OR Apache-2.0 | 2025-01-12 | 13.4M | keep |
| uuid | 1.27.0 | Apache-2.0 OR MIT | 2026-10-02 | 205.8M | keep |
| rand | 0.10.3 | MIT OR Apache-2.0 | 2026-09-20 | 495.7M | keep |
| once_cell | 1.21.4 | MIT OR Apache-2.0 | 2026-03-12 | 307.6M | keep, but std OnceLock/LazyLock now covers it |
| indexmap | 2.14.2 | Apache-2.0 OR MIT | 2026-09-05 | 394.7M | keep |
| smallvec | 1.16.2 (newest 2.0.0-beta.2) | MIT OR Apache-2.0 | 2026-09-25 | 312.2M | keep |
| crossbeam-channel | 0.5.17 | MIT OR Apache-2.0 | 2026-09-05 | 127.4M | keep |
| flume | 0.12.0 | Apache-2.0/MIT | 2025-12-08 | 62.4M | keep |
| sentry | 0.49.3 | MIT | 2026-09-21 | 13.8M | keep |
| tauri-plugin-sentry | 0.7.0 | MIT OR Apache-2.0 | 2026-09-28 | 250k | investigate - community plugin (timfish), not tauri-apps |

Pin warnings: `tauri` has `3.0.0-alpha.4` and most plugins have `3.0.0-alpha.x` published, so depend on `"2"` (cargo will not pick a prerelease for `"2"`). `specta`/`tauri-specta` have **no stable 2.x** - only `2.0.0-rc.25` - so Tauri v2 typed IPC needs an exactly pinned RC.

## 2. Frontend dependencies

| package | version | license | last release | verdict |
|---|---|---|---|---|
| react | 19.3.0 | MIT | 2026-09-09 | keep |
| react-dom | 19.3.0 | MIT | 2026-09-09 | keep |
| typescript | 7.0.2 | Apache-2.0 | 2026-07-08 | keep - note TS 7 is the native/Go port; new major, verify toolchain peers |
| vite | 8.3.4 | MIT | 2026-10-08 | keep - v8 major |
| @vitejs/plugin-react | 6.1.2 | MIT | 2026-10-05 | keep |
| @tanstack/react-router | 1.170.41 | MIT | 2026-09-30 | keep |
| @tanstack/react-query | 5.104.1 | MIT | 2026-10-02 | keep |
| @tanstack/react-virtual | 3.14.13 | MIT | 2026-09-14 | keep |
| @tanstack/react-table | 9.2.8 | MIT | 2026-10-08 | keep - v9 is a fresh major |
| zustand | 5.0.15 | MIT | 2026-08-13 | keep |
| tailwindcss | 4.3.3 | MIT | 2026-07-16 | keep |
| @tailwindcss/vite | 4.3.3 | MIT | 2026-07-16 | keep |
| motion | 14.0.0 | MIT | 2026-10-02 | keep |
| framer-motion | 14.0.0 | MIT | 2026-10-02 | drop - superseded by motion (same version train) |
| radix-ui | 1.7.0 | MIT | 2026-10-05 | keep - umbrella package; prefer it over individual @radix-ui/* pins |
| @radix-ui/react-dialog | 1.2.0 | MIT | 2026-10-05 | keep - fine if you need one-off primitives |
| class-variance-authority | 0.7.1 | Apache-2.0 | 2024-11-26 | keep - quiet since 2024 but stable and complete |
| tailwind-merge | 3.7.0 | MIT | 2026-09-12 | keep |
| clsx | 2.1.1 | MIT | 2024-04-23 | keep - quiet since 2024 but stable |
| cmdk | 1.1.1 | MIT | 2025-03-14 | keep |
| i18next | 26.4.2 | MIT | 2026-09-03 | keep |
| react-i18next | 17.0.16 | MIT | 2026-10-06 | keep |
| zod | 4.6.5 | MIT | 2026-09-13 | keep |
| react-hook-form | 7.89.0 | MIT | 2026-09-26 | keep |
| vitest | 5.0.3 | MIT | 2026-09-30 | keep |
| @testing-library/react | 16.3.3 | MIT | 2026-08-27 | keep |
| @testing-library/user-event | 14.6.7 | MIT | 2026-09-02 | keep |
| msw | 3.0.2 | MIT | 2026-10-03 | keep |
| playwright | 1.64.0 | Apache-2.0 | 2026-10-07 | keep |
| @playwright/test | 1.64.0 | Apache-2.0 | 2026-10-07 | keep |
| eslint | 10.12.0 | MIT | 2026-10-02 | keep - v10 major |
| typescript-eslint | 8.71.1 | MIT | 2026-10-05 | investigate - verify peer range against TypeScript 7 |
| prettier | 3.9.9 | MIT | 2026-09-23 | keep |
| knip | 6.40.0 | ISC | 2026-10-06 | keep |
| publint | 0.3.25 | MIT | 2026-10-01 | keep |
| size-limit | 14.2.0 | MIT | 2026-10-07 | keep |
| vite-plugin-svgr | 5.2.0 | MIT | 2026-04-02 | keep |
| lucide-react | 1.53.0 | ISC | 2026-10-08 | keep |
| @tauri-apps/api | 2.12.1 | Apache-2.0 OR MIT | 2026-09-30 | keep |
| @tauri-apps/cli | 2.12.1 | Apache-2.0 OR MIT | 2026-09-30 | keep |
| @tauri-apps/plugin-updater | 2.13.2 | MIT OR Apache-2.0 | 2026-10-07 | keep |
| @tauri-apps/plugin-store | 2.5.0 | MIT OR Apache-2.0 | 2026-09-26 | keep |
| sortablejs | 1.15.7 | MIT | 2026-02-11 | drop - not needed if using @dnd-kit |
| dnd-kit | 0.0.2 | MIT | 2024-01-11 | drop - 0.0.2 placeholder, not the real package. Use @dnd-kit/core |
| @dnd-kit/core | 6.3.1 | MIT | 2024-12-05 | keep - quiet since 2024-12, but stable |
| react-error-boundary | 6.1.6 | MIT | 2026-09-20 | keep |
| immer | 11.1.21 | MIT | 2026-10-02 | keep |
| suspend-react | 0.1.3 | MIT | 2023-06-13 | drop - abandoned since 2023-06 |

`dnd-kit` (0.0.2, 2024-01) is a placeholder squat; use `@dnd-kit/core`. `framer-motion` now publishes
in lockstep with `motion` (both 14.0.0) and is an alias, not a separate library.

## 3. Tools & CI

| tool | version | license | last release | signal (90d dl) | verdict |
|---|---|---|---|---|---|
| cargo-nextest | 0.9.148 | Apache-2.0 OR MIT | 2026-10-08 | 1.9M | keep |
| insta | 1.49.0 | Apache-2.0 | 2026-10-03 | 30.5M | keep |
| proptest | 1.11.0 | MIT OR Apache-2.0 | 2026-03-24 | 54.5M | keep |
| rstest | 0.27.0 | MIT OR Apache-2.0 | 2026-09-06 | 29.7M | keep |
| wiremock | 0.6.5 | MIT/Apache-2.0 | 2025-08-24 | 25.0M | keep, but httpmock releases more often |
| httpmock | 0.8.3 | MIT | 2026-02-04 | 6.4M | keep, but pick only one of wiremock/httpmock |
| tempfile | 3.27.0 | MIT OR Apache-2.0 | 2026-03-11 | 202.5M | keep |
| criterion | 0.8.2 | Apache-2.0 OR MIT | 2026-02-04 | 63.4M | keep |
| mockall | 0.15.0 | MIT OR Apache-2.0 | 2026-06-28 | 30.3M | keep |
| cargo-deny | 0.20.2 | MIT OR Apache-2.0 | 2026-07-09 | 1.9M | keep |
| cargo-audit | 0.22.2 | Apache-2.0 OR MIT | 2026-06-05 | 3.8M | keep |
| cargo-llvm-cov | 0.9.1 | Apache-2.0 OR MIT | 2026-09-06 | 1.8M | keep |
| git-cliff | 2.14.2 | MIT OR Apache-2.0 | 2026-09-18 | 49k | keep |
| release-plz | 0.3.170 | MIT OR Apache-2.0 | 2026-10-07 | 48k | keep |
| cargo-dist | 0.32.0 | MIT OR Apache-2.0 | 2026-05-22 | 19k | keep - crate name unchanged, tool renamed to dist |
| vergen | 10.0.3 | MIT OR Apache-2.0 | 2026-08-24 | 9.9M | keep (CI build metadata) |

`cargo-dist` renamed itself to `dist` ("[`dist` (formerly known as `cargo-dist`)](https://github.com/axodotdev/cargo-dist)"); the crates.io package is still `cargo-dist`, and the unrelated `dist` crate is a 2016 v0.0.0 placeholder. `cargo-audit` and `cargo-deny` overlap; running both is redundant.

## 4. Special investigations

### 4.1 librqbit - streaming torrent backend

**Verdict: keep.** 9.0.1, Apache-2.0, 2026-08-20; 132,899 lifetime / 60,484 90-day downloads; 1,927
stars on [ikatson/rqbit](https://github.com/ikatson/rqbit), pushed 2026-10-01, not archived.
License confirmed: `LICENSE` is verbatim Apache-2.0 and `crates/librqbit/Cargo.toml` declares
`license = "Apache-2.0"`, so your belief is right - GitHub's `NOASSERTION` is a detector artifact.

| capability | supported | evidence |
|---|---|---|
| sequential / streaming download | yes - sequential is the default **and only** mode | README features list; streaming prioritises the pieces being read |
| per-torrent file selection | yes | `AddTorrentOptions.only_files: Option<Vec<usize>>` / `only_files_regex`; later `Session::update_only_files(&Arc<Self>, &ManagedTorrentHandle, &HashSet<usize>)` |
| HTTP streaming API | yes, built in (axum) | `GET /torrents/{id_or_infohash}/stream/{file_idx}` honours `Range` for seek; `HttpApi::new(api, opts)` embeds in your router |
| SOCKS5 + interface binding | yes, both | `--socks-url` / `RQBIT_SOCKS_PROXY_URL` (`socks5://`, `socks5h://`); `SessionOptions.bind_device_name` (`IP_BOUND_IF` macOS, `SO_BINDTODEVICE` Linux) |
| DHT / PEX / LSD / uTP | yes; uTP **listening** is experimental | BEP-5, BEP-11, BEP-14; `librqbit-utp` is a hard dependency; CLI flag is `--experimental-enable-utp-listen` |
| RSS / seeding policy | **no RSS; minimal seeding** | no `rss` in `crates/rqbit/src/main.rs`; use the `watch` feature + an external reader. Only `--disable-upload` (feature-gated) and `--ratelimit-upload`; no ratio or seed-time policy |
| other limits | no transcoding; not magnet-only | `--enable-upnp-server` streams without transcoding; `AddTorrent::from_url` takes magnet, `.torrent` URL, local path or raw bytes |

Exact API for adding a torrent and reading a stream:

```rust
let session = Session::new(PathBuf::from("/downloads")).await?;          // Arc<Session>
let resp = session.add_torrent(AddTorrent::from_url(magnet), Some(AddTorrentOptions {
    only_files: Some(vec![1]), overwrite: true, ..Default::default() })).await?;
let handle = resp.into_handle().expect("magnet");   // ManagedTorrentHandle = Arc<ManagedTorrent>
let mut stream = handle.stream(1).await?;           // FileStream: AsyncRead + AsyncSeek
```

`ManagedTorrentHandle` is the type alias at `crates/librqbit/src/torrent_state/mod.rs:650`, which is
why `stream()` is callable directly on the handle. Risks: no ratio/seed-time policy (write it over
`TorrentStats`); sequential-only removes rarest-first tuning; do not copy rqbit's `panic = "abort"`
release profile into a Tauri app.

### 4.2 Embedding mpv

**Verdict: ship mpv as a separate executable driven over JSON IPC.** Only design that is uniform
across all four targets and keeps licensing trivial; treat in-window embedding as an enhancement.

| platform | `--wid` | libmpv render API | borderless/child window | recommendation |
|---|---|---|---|---|
| macOS | **no** | yes - Metal/GL into your FBO or an `NSView` | yes, you position it | child window, else render API |
| Windows | yes - `HWND` as `uint32_t`; mpv reparents and fills it | yes - D3D11/ANGLE | yes - `WS_CHILD`/owned popup | `--wid` on a child window |
| Linux X11 | yes - `Window`; mpv sets it as parent | yes - GLX/EGL | yes | `--wid` |
| Linux Wayland | **no** | yes - into your FBO, or a software buffer | **no** | render API, or a standalone mpv toplevel |

- [`--wid` in `DOCS/man/options.rst`](https://github.com/mpv-player/mpv/blob/master/DOCS/man/options.rst) documents only X11, win32 and Android; X11 implements it (`x11_common.c` reads `opts->WinID` and reparents).
- [`cocoa_common.m`](https://github.com/mpv-player/mpv/blob/master/video/out/cocoa_common.m) and [`wayland_common.c`](https://github.com/mpv-player/mpv/blob/master/video/out/wayland_common.c) contain no `WinID` reference at all - macOS and Wayland `--wid` are unimplemented.

**Wayland limitation.** Two independent blockers: mpv has no Wayland `--wid`, and Wayland has no protocol for one client to embed another client's surface and does not let a client position itself (mpv's docs say so for several window options). So "float a borderless mpv window exactly over the app window" is unavailable on Wayland too. Fallbacks, best first:

1. **In-process render API.** Create a `wl_subsurface` from Tauri's GTK parent surface (same client, so legal), bind an EGL context, create an mpv render context with `MPV_RENDER_PARAM_API_TYPE` + `MPV_RENDER_PARAM_WL_DISPLAY`, let mpv render into `MPV_RENDER_PARAM_OPENGL_FBO`, present that.
2. **Software render.** `mpv_render_context_render` with `SW_SIZE`/`SW_FORMAT`/`SW_STRIDE`/`SW_POINTER` writes frames into memory you own; blit into the webview. Slower, but uniform on all platforms.
3. **Degraded hand-off.** mpv opens a normal toplevel (`--vo=gpu --gpu-context=wayland`) and the webview hides ("theatre mode"). Ugly, free, zero-risk.

- Tauri side: `tauri::Window` implements `HasWindowHandle`, so `window_handle()` yields HWND / Xlib window / NSView / `wl_surface`, and `gtk_window()` gives the GTK window on Linux ([docs.rs](https://docs.rs/tauri/latest/tauri/window/struct.Window.html)).
- Caveat on Windows/X11: the WebView2/WebKitGTK child window and mpv's child window are siblings, so you need explicit resize/move sync plus a hole or explicit raise/lower - not just `--wid`.

**Licensing.** mpv is **GPL-2.0-or-later** by default, **LGPL-2.1-or-later** only when built without
any GPL-only file via the Meson switch `-Dgpl=false` ([`Copyright`](https://github.com/mpv-player/mpv/blob/master/Copyright)).

- Upstream: the switch "does not in itself create a LGPLv2.1+ license grant".
- LGPL mode **disables** Linux X11 video output (`vo_x11.c`, `vo_xv.c`, `x11_common.*` are GPL-only), VDPAU (`vo_vdpau.c`, `vo_vaapi.c`), BSD OSS audio, JACK, DVD/CDDA/DVB, CACA and legacy Direct3D. The X11 point decides it: a strict LGPL-only mpv build has **no X11 output**.
- Upstream: LGPL mode is for libmpv and "it's not recommended to build mpv CLI in LGPL mode at all".
- **Separate executable (recommended).** mpv's license does not reach your code - a GPL "aggregate" of separate programs ([GPL FAQ](https://www.gnu.org/licenses/gpl-faq.html#MereAggregation)). Drive it over JSON IPC on a unix socket / named pipe ([`DOCS/man/ipc.rst`](https://github.com/mpv-player/mpv/blob/master/DOCS/man/ipc.rst), `--input-ipc-server`). Obligations: ship mpv's license files and a source offer for the exact binary; you may ship the ordinary GPL build and keep X11/VDPAU.
- **Linking libmpv (avoid).** LGPLv2.1 permits it from a permissive app if you meet [section 6](https://www.gnu.org/licenses/old-licenses/lgpl-2.1.html#SEC6) (dynamic linking plus the user's ability to relink a modified libmpv). The trap: the `libmpv2` Rust crate is itself **LGPL-2.1** and Cargo links crates **statically** by default, pulling LGPL code into your binary. If you need it, load `libmpv` at runtime with `libloading` instead.

### 4.3 Chromecast and DLNA from Rust

**Verdict: `rust_cast` is usable but is only a transport; do not use `ssdp`; there is no `dlna` crate;
use `rupnp` plus hand-rolled SOAP.**

- `rust_cast` 0.21.0, MIT, 2025-12-30, 156,706 lifetime / 26,038 90-day downloads; [158 stars](https://github.com/azasypkin/rust-cast), 4 open issues, pushed 2025-12-30 - marginally maintained. 0.20.0 and 0.21.0 landed the same day, which reads as a maintenance burst.
- Covers connection, heartbeat, receiver (`LAUNCH`/`STOP`/`GET_STATUS`/`SET_VOLUME`; default receiver `CC1AD845`) and media: `LOAD`, `QUEUE_LOAD`, `PLAY`, `PAUSE`, `STOP`, `SEEK`, `GET_STATUS`, `MEDIA_STATUS`.
- **Gaps:** no device discovery (the README tells you to use `avahi-browse`); **no track/subtitle API** - `EDIT_TRACKS`, `activeTrackIds` and `textTrackStyle` do not appear in `src/channels/media.rs`; no generic custom namespace; no Cast Groups.

| DLNA option | version | license | last release | 90d dl | verdict |
|---|---|---|---|---|---|
| `dlna` | **does not exist** | - | - | - | not an option; name is free on crates.io |
| `dlna-rs` | 0.1.0 | - | 2026-09-13 | 13 | too new |
| `ssdp` | 0.7.0 | MIT/Apache-2.0 | 2018-10-05 | 710 | dead, 8 years |
| `rupnp` | 3.0.0 | MIT/Apache-2.0 | 2025-04-25 | 49,886 | **recommended**: `ssdp_client` discovery + `Device`/`Service` SOAP |
| `upnp-client` | 0.1.11 | - | 2025-06-19 | 1,279 | workable fallback |
| `igd-next` | 0.18.0 | MIT | 2026-10-04 | 4.5M | **not DLNA** - router port mapping only |
| `mdns-sd` | 0.21.5 | Apache-2.0 OR MIT | 2026-10-05 | 2.7M | Chromecast discovery (`_googlecast._tcp.local`); DLNA uses SSDP, not mDNS |

DLNA plan: `rupnp::ssdp_client` to find MediaRenderers, then hand-roll only the SOAP you need - `AVTransport` (`SetAVTransportURI`, `Play`, `Pause`, `Seek`, `GetTransportInfo`), `RenderingControl`, and `ConnectionManager.GetProtocolInfo` to learn what the renderer accepts. No maintained crate does this for you; budget it as real work.

Chromecast, from [Google's Supported Media for Google Cast](https://developers.google.com/cast/docs/media): the container list is **MP2T, MP3, MP4, OGG, WAV, WebM** - Matroska is absent. Delivery methods are MPEG-DASH, SmoothStreaming, HLS and progressive download; adaptive protocols and **all subtitle resources require CORS**.

| source | direct play | action |
|---|---|---|
| H.264+AAC in MP4; VP8/VP9+Vorbis in WebM | yes | serve as-is |
| H.264 in MKV | no | remux to MP4 or fMP4-HLS, `-c copy` |
| H.265 in MKV | no | remux into **MP4/CMAF**; Google warns "HEVC is not supported on Transport Stream containers", so no TS segments |
| H.265 on Chromecast 1st/2nd gen | no | transcode to H.264 (those devices list only H.264 HP + VP8) |
| H.265 on Ultra / with Google TV / Streamer | yes | HEVC Main/Main10 (AV1 on newer); MP4 container required |
| AC-3 / E-AC-3 | conditional | passthrough only, via the Web Receiver SDK; probe with `CastReceiverContext.canDisplayType()` |
| DTS / TrueHD | no | transcode to AAC (same as any unsupported audio) |
| subtitles (TTML, WebVTT, CEA-608/708) | yes | convert ASS/SSA to WebVTT; serve with CORS |

Baseline rule: **HLS with fMP4 segments and H.264+AAC**. H.264 source needs only `-c:v copy`; everything else needs a remux or a real transcode. The app must serve media from its own local HTTP server with `Accept-Ranges` and CORS - the receiver pulls the URL, it is not pushed bytes. Residual risk: receiver-side subtitle selection means writing `EDIT_TRACKS` yourself, and DLNA renderers routinely advertise protocols in `GetProtocolInfo` that they cannot actually play.

### 4.4 ffmpeg licensing

[ffmpeg.org/legal.html](https://ffmpeg.org/legal.html): FFmpeg is **LGPL-2.1-or-later** by default; optional parts are **GPL-2.0-or-later** and "if those parts get used the GPL applies to all of FFmpeg". Checklist item 1: compile **without `--enable-gpl`** and **without `--enable-nonfree`**. A third switch, `--enable-version3`, upgrades (L)GPL to version 3.

- GPL-only external libraries, from [`configure`](https://github.com/FFmpeg/FFmpeg/blob/master/configure) (`EXTERNAL_LIBRARY_GPL_LIST`): `libx264`, `libx265`, `libxvid`, `libxavs`, `libxavs2`, `libdavs2`, `libdvdnav`, `libdvdread`, `librubberband`, `libvidstab`, `libcdio`, `frei0r`, `avisynth`.
- LGPL-safe: remux MKV->MP4 (`-c copy`, no encoder); subtitle conversion; AC-3/DTS -> AAC (ffmpeg's native AAC encoder is LGPL); H.264 transcode via hardware encoders (NVENC's ffmpeg source is LGPL; VideoToolbox, QSV, AMF, VAAPI likewise) or via `--enable-libopenh264`, which is in `EXTERNAL_LIBRARY_LIST`, not the GPL list.
- Not LGPL-safe: H.264 transcode through **libx264**, which needs `--enable-gpl` and makes that binary GPL.
- `libopenh264` is BSD-licensed, but Cisco's gratis binary carries the MPEG-LA patent coverage while a self-built one does not automatically inherit it - a patent question, separate from copyright.
- **Recommended design:** bundle `ffmpeg`/`ffprobe` as **sidecar executables** and never link libav*. Use Tauri's official sidecar mechanism (`externalBin` + [shell-plugin sidecar API](https://v2.tauri.app/develop/sidecar/)) so the binaries are signed and versioned with the app. A separate program invoked as a subprocess is an aggregate, not a derivative work ([GPL FAQ](https://www.gnu.org/licenses/gpl-faq.html#MereAggregation)).
- Obligations: exact corresponding source for the binary you ship (tarball/zip on the same webserver as the binary), the configure line and a `git diff` if you patched, credit in the about box and on any download page, no obfuscated DLL renames, no EULA ban on reverse engineering, plus the GPL source offer if the build is GPL. Budget ~70-80 MB per platform for the pair; macOS notarization and Windows signing must now cover two more binaries.

### 4.5 Media probe: ffprobe sidecar versus pure Rust

**Verdict: ffprobe sidecar for anything authoritative; `lofty` for audio tags; `matroska` for
in-process MKV structure.**

| field | ffprobe | lofty | matroska | mp4parse | mp4 | symphonia |
|---|---|---|---|---|---|---|
| container + duration | yes | partial (audio-oriented) | yes (`Info`) | yes | yes | partial |
| video codec id / profile / level | yes | no | partial (codec id only) | partial | partial | no |
| bit depth / pixel format / chroma | yes | no | partial (audio bit depth only) | partial | partial | no |
| colour primaries / transfer / matrix | yes | no | no | yes (`ColorSpace`) | no | no |
| HDR mastering metadata (ST 2086, MaxCLL/MaxFALL) | yes | no | no | no | no | no |
| HDR10+ / Dolby Vision dynamic metadata | yes | no | no | no | no | no |
| audio codec / channels / layout / rate / depth | yes | yes | yes (`Audio`) | yes | partial | yes |
| embedded subtitles (lang, codec, forced, text vs PGS) | yes | no | yes (`Track`, `Tracktype::Subtitle`) | partial | partial | no |
| chapters with timestamps + titles | yes (`-show_chapters`) | no | yes (`Chapter`) | no | no | no |
| timecodes / start offsets | yes | partial | yes (`Info` timestamp scale) | partial | partial | no |
| stream tags / metadata | yes | yes | yes (`Tag`, `Target`) | no | yes | partial |

- [ffprobe options](https://ffmpeg.org/ffprobe.html): `-show_streams`, `-show_format`, `-show_chapters`, `-show_frames`.
- The HDR rows come from FFmpeg's frame side-data enum in [`libavutil/frame.h`](https://github.com/FFmpeg/FFmpeg/blob/master/libavutil/frame.h) (`AV_FRAME_DATA_MASTERING_DISPLAY_METADATA`, `..._CONTENT_LIGHT_LEVEL`, `..._DYNAMIC_HDR_PLUS`, `..._DOVI_RPU_BUFFER`, `..._DOVI_METADATA`, `..._ICC_PROFILE`), surfaced as `side_data_list`; no pure-Rust parser here exposes them.
- [`lofty`'s `properties`](https://docs.rs/lofty/latest/lofty/properties/index.html) self-describes as "generic **audio** properties ... common to all audio formats" - no chapter or video-codec surface.
- [`matroska`](https://docs.rs/matroska/latest/matroska/) gives `Matroska`/`Info`/`Track`/`Audio`/`Video`/`Chapter`/`Tag`/`Attachment`, but `Video` carries only `pixel_width`, `pixel_height`, `display_width`, `display_height`, `interlaced`, `stereo`, `gamma` - no colour or mastering metadata.
- Ecosystem: `matroska` 0.30.1 and `matroska-demuxer` 0.8.1 are both alive; `mp4parse` 0.17.0 (MPL-2.0) and `mp4` 0.14.0 (MIT) have not shipped since 2023; `symphonia` 0.6.1 is a decoder framework, not a probe.
- Sidecar cost: one spawn per probe (order 10-50 ms - batch and cache) plus the binary size from 4.4. Pure Rust alone cannot detect HDR or Dolby Vision, exactly the metadata a media app wants to display.

## 5. Licensing summary for redistribution

| component | what we ship | license | obligations |
|---|---|---|---|
| Peregrine app code | executable | MIT / Apache-2.0 (your choice) | none beyond your own license |
| Rust crates (bulk) | static in binary | MIT / Apache-2.0 | ship a generated third-party license list (`cargo-deny`/`cargo-about`); Apache-2.0 requires including any NOTICE file |
| `encoding_rs` | static | (Apache-2.0 OR MIT) AND BSD-3-Clause | retain the BSD-3-Clause notice; no-endorsement clause |
| `moka` | static | (MIT OR Apache-2.0) AND Apache-2.0 | attribution |
| `walkdir` / `notify` | static | Unlicense+MIT / CC0-1.0 | attribution for walkdir; none for notify |
| `mp4parse`, `symphonia`, `subparse` (if adopted) | static | MPL-2.0 | file-level copyleft: publish changes to MPL files; the larger work may stay MIT/Apache |
| `libmpv2` (if adopted) | static - LGPL code in binary | **LGPL-2.1** | LGPLv2.1 s6 relink obligation; prefer runtime `libloading`, or do not link |
| `mpv-client` (if adopted) | static | **GPL-3.0** | would force GPL on the whole app. Do not link. |
| mpv | separate executable | GPL-2.0-or-later (LGPL-2.1+ only with `-Dgpl=false`) | ship mpv's license text and a source offer for the exact binary; an LGPL-only build loses X11 output |
| ffmpeg / ffprobe | separate executables | LGPL-2.1+ (GPL-2.0+ if `--enable-gpl`) | FFmpeg checklist: corresponding source on the same webserver, configure line, `git diff` if patched, about-box and download-page credit, no obfuscated DLL renames, no EULA reverse-engineering ban; a GPL build adds the GPL source offer |
| Chromecast receiver | Google-hosted default receiver | Google ToS | none for the default receiver; a custom receiver does |
| fonts / icons (`lucide-react`) | bundled | ISC | attribution |
| JS packages | bundled frontend | MIT / Apache-2.0 / ISC | bundle a license list in the about dialog; eslint/playwright/vitest are dev-only, not redistributed |

Copyleft boundary: everything statically linked into the binary is MIT/Apache/BSD/ISC plus optionally MPL-2.0 files; every GPL or LGPL component (mpv, ffmpeg, and `mpv-client` if you were tempted) stays outside the process boundary as a separate executable.

## 6. Unverified

- `dlna` crate: confirmed **absent** from crates.io (`crate 'dlna' does not exist`). Do not plan around it.
- `sortablejs`: published (1.15.7, 2026-02-11, MIT) but the npm package's provenance was not cross-checked against the SortableJS GitHub project. Moot if `@dnd-kit` is used.
- `suspend-react` 0.1.3 (2023-06-13): metadata verified, no maintenance signal beyond the stale date; treat as abandoned.
- GitHub star counts were collected only for librqbit, rust-cast, mpv, libmpv-rs, cargo-dist, mdns-sd, rupnp, sentry-tauri, tauri-specta, matroska, matroska-demuxer, mp4parse-rust, Symphonia, lofty-rs, ffmpeg-sidecar, ass_parser and subparse. The GitHub REST rate limit was exhausted mid-run, so other verdicts rest on crates.io/npm release and download signals alone.
- librqbit SOCKS5 coverage of **UDP tracker** traffic (versus TCP) was not verified in source; only the documented `--socks-url` schemes are confirmed. Test before relying on it for leak prevention.
- `libmpv2` ABI/API pairing against the specific mpv build you bundle was not verified; it is version-coupled to libmpv and must be tested against your pinned release.
- Whether `typescript-eslint` 8.71.1 declares a peer range accepting TypeScript 7.0.2 was not checked against the package manifest.
- Whether `cargo-dist` 0.32.0 still installs a `cargo-dist` binary alongside `dist` was not verified.
- GitHub reported `NOASSERTION` for the rqbit license; resolved by reading `LICENSE` and `Cargo.toml` directly.
