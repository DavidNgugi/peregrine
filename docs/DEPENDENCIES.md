# Peregrine — Dependencies

Pinned versions, licences, and the obligations that come with redistributing them. Every version below was resolved live against crates.io and npm on **2026-10-08** with `User-Agent: peregrine-plan`; the full evidence, download signals, and unverified items are in [research/dependency-matrix.md](./research/dependency-matrix.md). Stack rationale is in [TECH_STACK.md](./TECH_STACK.md); licence decisions in [adr/0009](./adr/0009-licensing-and-redistribution.md), [adr/0013](./adr/0013-mpv-distribution-and-video-surface.md), and [adr/0014](./adr/0014-ffmpeg-build-and-transcode.md).

## 1. Version policy

| Rule | Detail |
| --- | --- |
| Lockfiles | `Cargo.lock` and `pnpm-lock.yaml` are committed. CI builds with `--locked`. |
| Rust ranges | `"2"`-style ranges for Tauri and its plugins (cargo does not select the published `3.0.0-alpha.x` prereleases for a `"2"` requirement), exact pins for anything whose API we depend on closely. |
| Exact pins | `specta` and `tauri-specta` at `=2.0.0-rc.25`: no stable 2.x exists, so the RC is deliberate and pinned exactly. |
| MSRV | Stable Rust minus two releases, declared in `Cargo.toml` and enforced in CI. |
| Node | Node 22 LTS or newer, pnpm 10 or newer, declared in `engines` and used in CI. |
| Updates | Renovate, grouped weekly. A major needs a PR with a CI pass plus a visual regression run for frontend majors. |
| Sidecars | Exact version plus SHA-256 per OS, upgraded by a scheduled PR that runs the media fixture suite. |
| New dependency | Needs a stated reason in the PR and a licence check against the `cargo-deny` allowlist. |

## 2. Rust runtime dependencies

| Crate | Version | Licence | Purpose |
| --- | --- | --- | --- |
| `tauri` | 2.12.1 | Apache-2.0 OR MIT | Application shell, IPC, window and menu integration |
| `tauri-build` | 2.7.1 | Apache-2.0 OR MIT | Build script and capability processing |
| `tauri-plugin-{updater, store, window-state, single-instance, os, notification, deep-link, shell, opener, dialog, fs, http, log, global-shortcut, clipboard-manager, process, autostart}` | 2.4–2.13 | Apache-2.0 OR MIT | Platform integration; each used for exactly one documented purpose ([ARCHITECTURE](./ARCHITECTURE.md) §17) |
| `specta` + `tauri-specta` | **=2.0.0-rc.25** | MIT | Rust-to-TypeScript type export for the IPC contract |
| `tokio` | 1.53.2 | MIT | Async runtime |
| `tokio-util` | 0.7.19 | MIT | `CancellationToken`, IO adapters |
| `futures` | 0.3.34 | MIT OR Apache-2.0 | Stream combinators |
| `serde`, `serde_json` | 1.0.229, 1.0.151 | MIT OR Apache-2.0 | Serialization for IPC and provider payloads |
| `thiserror` | 2.0.21 | MIT OR Apache-2.0 | Typed library errors |
| `anyhow` | 1.0.104 | MIT OR Apache-2.0 | Context at the application boundary only |
| `tracing`, `tracing-subscriber`, `tracing-appender` | 0.1.44, 0.3.23, 0.2.5 | MIT | Structured logs, redaction layer, rotation |
| `sqlx` | 0.9.0 | MIT OR Apache-2.0 | SQLite access, migrations, compile-time-checked queries |
| `librqbit` | 9.0.1 | **Apache-2.0** (verified in `LICENSE` and `Cargo.toml`; GitHub's `NOASSERTION` is a detector artifact) | BitTorrent engine: sequential streaming, per-file selection, DHT/PEX/LSD, SOCKS5 and interface binding, built-in Range streaming API |
| `axum` + `tower-http` | 0.8.9, 0.7.1 | MIT | Loopback and LAN media, subtitle, and HLS serving |
| `hyper`, `tower` | 1.12.0, 0.5.3 | MIT | Server plumbing |
| `reqwest` | 0.13.5 | MIT OR Apache-2.0 | Provider HTTP with rustls and HTTP/2 |
| `reqwest-middleware`, `reqwest-retry` | 0.5.2, 0.9.1 | MIT OR Apache-2.0 | Retry and conditional-request middleware |
| `governor` | 0.10.4 | MIT | Per-host token buckets |
| `backon` | 1.6.0 | Apache-2.0 | Exponential backoff with jitter. Replaces `backoff`, abandoned since 2021. Version and licence were resolved and checked live on 2026-10-09; the earlier "MIT OR Apache-2.0" here was wrong. Not yet used — it is added with the provider retry work in Phase 1 |
| `moka` | 0.12.16 | (MIT OR Apache-2.0) AND Apache-2.0 | Bounded in-memory LRU |
| `dashmap` | 6.2.1 | MIT | Concurrent maps for session and job registries |
| `ulid` | 3.0.0 | MIT | Sortable IDs for jobs and events |
| `jiff` | 0.2.38 | Unlicense OR MIT | Time zones, DST, air dates |
| `chardetng`, `encoding_rs` | 1.0.0, 0.8.42 | Apache-2.0 OR MIT; (Apache-2.0 OR MIT) AND BSD-3-Clause | Subtitle charset detection and conversion — retain the BSD notice for `encoding_rs` |
| `image`, `fast_image_resize`, `blurhash` | 0.25.10, 6.1.0, 0.2.3 | MIT OR Apache-2.0; MIT OR Apache-2.0; Apache-2.0/MIT | Decode, resize to display variants, generate placeholders |
| `jwalk`, `walkdir`, `notify`, `fs4`, `sysinfo` | 0.9.0, 2.5.0, 8.2.0, 1.1.0, 0.39.6 | MIT; Unlicense/MIT; CC0-1.0; MIT OR Apache-2.0; MIT | Parallel library scan, watcher, free space, resource stats |
| `keyring`, `secrecy`, `zeroize` | 4.2.0, 0.10.3, 1.9.1 | MIT OR Apache-2.0; Apache-2.0 OR MIT | OS keyring storage and in-memory secret protection |
| `mdns-sd` | 0.21.5 | Apache-2.0 OR MIT | Chromecast discovery on `_googlecast._tcp` |
| `rupnp` | 3.0.0 | MIT OR Apache-2.0 | DLNA discovery (SSDP) and SOAP invocation |
| `rust_cast` | 0.21.0 | MIT | Chromecast CASTV2 transport. **No discovery and no track/subtitle API** — both are ours |
| `raw-window-handle` | 0.6.2 | MIT OR Apache-2.0 OR Zlib | Obtaining the platform window handle for the video surface |
| `ffmpeg-sidecar` | 2.6.0 | MIT | Locating, invoking, and parsing progress from the ffmpeg and ffprobe sidecars |
| `sentry` | 0.49.3 | MIT | Opt-in crash reporting with an empty default DSN |
| `directories` | 6.0.0 | MIT OR Apache-2.0 | Per-OS config, cache, and data paths |
| `uuid`, `rand`, `indexmap`, `smallvec`, `flume` | 1.27.0, 0.10.3, 2.14.2, 1.16.2, 0.12.0 | Apache-2.0 OR MIT; MIT OR Apache-2.0; Apache-2.0 OR MIT; MIT OR Apache-2.0; Apache-2.0/MIT | Identifiers, jitter, ordered maps, small buffers, channels |

### Deliberately not used

| Rejected | Reason | Used instead |
| --- | --- | --- |
| `libmpv2` | The crate is LGPL-2.1 and Cargo links crates statically, which would put LGPL code inside our binary | mpv as a separate executable ([adr/0013](./adr/0013-mpv-distribution-and-video-surface.md)) |
| `mpv-client` | GPL-3.0; linking it would force the whole application to GPL | JSON IPC over a socket, hand-rolled (~300 lines) |
| `ssdp` | Dead since 2018-10-05 | `rupnp` |
| `dlna` | **Does not exist on crates.io** | `rupnp` plus hand-rolled `AVTransport`/`RenderingControl` SOAP |
| `rusqlite`, `refinery` | Redundant with `sqlx`, which covers SQLite and migrations in one dependency | `sqlx` |
| `chrono`, `time` | `jiff` covers zones and DST better, and a second time crate is a second set of bugs | `jiff` |
| `backoff` | Abandoned since 2021; `librqbit` already uses `backon` | `backon` |
| `once_cell` | `std::sync::OnceLock` and `LazyLock` are stable | std |
| `opener` | `tauri-plugin-opener` already covers it | `tauri-plugin-opener` |
| `srt`, `srt-rs` | `srt` is a 2017 stub; `srt-rs` is libsrt transport bindings, not subtitle parsing | Own SRT/VTT/ASS readers and writers in `peregrine-domain` (`PG-E4-15`), per the module map in [ARCHITECTURE](./ARCHITECTURE.md) §5 |
| `subparse`, `ass-parser` | `subparse` has had no release since 2021 and carries MPL-2.0 file-level obligations; `ass-parser` has 6 stars | Own readers and writers (the formats are simple text; ~600 lines with tests), plus `chardetng` and `encoding_rs` for encoding |
| `lofty` as a probe | Audio-only properties; no video codec, colour, or mastering metadata | `ffprobe` sidecar |
| Media Player Classic / guliverkli movie-hash source | GPL-derived; porting it would contaminate our code | Reimplemented from the published algorithm description, whose test vectors are public data |
| A bundled `yt-dlp` | The PyInstaller release binaries include GPLv3+ code, so bundling one makes the distribution GPLv3+ | Off by default; a **user-installed** `yt-dlp` invoked as a separate process, with the ToS and copyright caveats documented |
| Podnapisi as a subtitle provider | Its host no longer resolves, and it sits behind a Cloudflare challenge | OpenSubtitles and Subdl only |
| `matroska` | Cannot report HDR mastering metadata | `ffprobe` sidecar; `matroska` only if a cheap in-process MKV check is ever needed |

## 3. Frontend dependencies

| Package | Version | Licence | Purpose |
| --- | --- | --- | --- |
| `react`, `react-dom` | 19.3.0 | MIT | UI runtime |
| `typescript` | 7.0.2 | Apache-2.0 | Types. **New major (the native port)** — verify the whole toolchain together at Phase 0 |
| `vite`, `@vitejs/plugin-react` | 8.3.4, 6.1.2 | MIT | Build and HMR |
| `@tanstack/react-router` | 1.170.41 | MIT | Type-safe routes and search params |
| `@tanstack/react-query` | 5.104.1 | MIT | Server-state cache, invalidation, optimistic updates |
| `@tanstack/react-virtual` | 3.14.13 | MIT | Grid and rail virtualization |
| `@tanstack/react-table` | 9.2.8 | MIT | Dense tables (queue, files, tracks). Fresh major — verify at Phase 0 |
| `zustand` | 5.0.15 | MIT | Ephemeral UI state |
| `tailwindcss`, `@tailwindcss/vite` | 4.3.3 | MIT | Token-driven styling with zero runtime |
| `motion` | 14.0.0 | MIT | Shared-element and layout transitions. `framer-motion` is now an alias of the same version train and is not installed |
| `radix-ui` | 1.7.0 | MIT | Behaviour and accessibility primitives via the umbrella package rather than individual `@radix-ui/*` pins |
| `class-variance-authority`, `tailwind-merge`, `clsx` | 0.7.1, 3.7.0, 2.1.1 | Apache-2.0; MIT | Variant and class composition |
| `cmdk` | 1.1.1 | MIT | Command palette |
| `i18next`, `react-i18next` | 26.4.2, 17.0.16 | MIT | Messages, plurals, locale formatting |
| `zod`, `react-hook-form` | 4.6.5, 7.89.0 | MIT | Settings-form validation |
| `@dnd-kit/core` | 6.3.1 | MIT | Drag and drop with keyboard alternatives. `dnd-kit` (0.0.2) is a placeholder squat and `sortablejs` is unnecessary |
| `react-error-boundary` | 6.1.6 | MIT | Error isolation per feature |
| `immer` | 11.1.21 | MIT | Immutable updates in Zustand slices |
| `lucide-react` | 1.53.0 | ISC | Icon set; attribution required |
| `@tauri-apps/api`, `@tauri-apps/cli` | 2.12.1 | Apache-2.0 OR MIT | Frontend bridge and CLI |
| `@tauri-apps/plugin-updater`, `@tauri-apps/plugin-store` | 2.13.2, 2.5.0 | MIT OR Apache-2.0 | Update prompt and small client-side persistence |
| `vite-plugin-svgr` | 5.2.0 | MIT | SVG-as-component for icons |

Dropped: `framer-motion` (alias of `motion`), `suspend-react` (abandoned 2023), `sortablejs` (superseded by `@dnd-kit/core`).

## 4. Development and CI tools

| Tool | Version | Licence | Purpose |
| --- | --- | --- | --- |
| `cargo-nextest` | 0.9.148 | Apache-2.0 OR MIT | Test runner |
| `insta` | 1.49.0 | Apache-2.0 | Snapshot tests for provider normalization and parsing |
| `proptest` | 1.11.0 | MIT OR Apache-2.0 | Property tests for parsers, path guards, ranking |
| `rstest` | 0.27.0 | MIT OR Apache-2.0 | Fixture-parameterized tests |
| `wiremock` | 0.6.5 | MIT/Apache-2.0 | Provider HTTP mocks. One of `wiremock`/`httpmock` only — `wiremock` chosen for its `insta`-friendly ergonomics |
| `tempfile` | 3.27.0 | MIT OR Apache-2.0 | Temporary SQLite databases and roots |
| `criterion` | 0.8.2 | Apache-2.0 OR MIT | Benches behind the budget gate |
| `mockall` | 0.15.0 | MIT OR Apache-2.0 | Port mocks where fakes are insufficient |
| `toml` | 1.1.7 | MIT OR Apache-2.0 | Parsing `Cargo.toml` and the architecture allowlist in `xtask`. Dev-only, never shipped. Added with `PG-E0-04`: the arch gate has to read manifests, and a hand-rolled TOML parser in a gate is a correctness risk |
| `sha2` | 0.11.0 | MIT OR Apache-2.0 | SHA-256 for the migration journal in `xtask`. Dev-only, never shipped. Added with `PG-E0-05`: a hand-rolled hash in a gate is a correctness risk |
| `cargo-deny` | 0.20.2 | MIT OR Apache-2.0 | Licence allowlist, bans, advisories — the primary supply-chain gate |
| `cargo-audit` | 0.22.2 | Apache-2.0 OR MIT | Secondary advisory check; overlaps `cargo-deny`, run weekly rather than per-commit |
| `cargo-llvm-cov` | 0.9.1 | Apache-2.0 OR MIT | Coverage with per-crate thresholds |
| `git-cliff` | 2.14.2 | MIT OR Apache-2.0 | Changelog generation |
| `release-plz` | 0.3.170 | MIT OR Apache-2.0 | Release PRs |
| `vergen` | 10.0.3 | MIT OR Apache-2.0 | Git metadata in the About box and diagnostics |
| `vitest`, `@testing-library/react`, `@testing-library/user-event`, `msw` | 5.0.3, 16.3.3, 14.6.7, 3.0.2 | MIT | Frontend unit tests with a mocked IPC bridge |
| `@playwright/test` | 1.64.0 | Apache-2.0 | E2E flows and frame-time measurement |
| `eslint`, `typescript-eslint`, `prettier` | 10.12.0, 8.71.1, 3.9.9 | MIT | Lint and format. Verify `typescript-eslint`'s peer range against TypeScript 7 before Phase 0 |
| `knip`, `publint`, `size-limit` | 6.40.0, 0.3.25, 14.2.0 | ISC; MIT | Dead code, package hygiene, bundle budgets |

`toml` and `sha2` were resolved and licence-checked live against crates.io on **2026-10-09** with `User-Agent: peregrine-plan`; both are dev-only and appear in no shipped binary. Every other version above was resolved on **2026-10-08**.

Only `cargo-dist` (renamed upstream to `dist`) was considered and is **not** adopted: `tauri-action` plus the Tauri bundler already produces every artifact we ship, and a second release tool would duplicate it.

## 5. Bundled sidecars

| Component | Version | Licence | Why this build |
| --- | --- | --- | --- |
| `mpv` | pinned per OS | **GPL-2.0-or-later** (upstream's ordinary build) | An LGPL-only build (`-Dgpl=false`) disables `vo_x11.c`, `vo_xv.c`, and `x11_common.*`, which means no X11 video output at all. Shipped as a separate executable, which is an aggregate rather than a derivative work ([adr/0013](./adr/0013-mpv-distribution-and-video-surface.md)) |
| `ffmpeg` | pinned per OS | **LGPL-2.1-or-later** (`--enable-gpl`, `--enable-nonfree`, and `--enable-version3` all absent) | Full remux, subtitle, and AAC encode capability without copyleft. Consequence: no libx264, so casting transcodes with hardware encoders or `libopenh264` ([adr/0014](./adr/0014-ffmpeg-build-and-transcode.md)) |
| `ffprobe` | same build as ffmpeg | LGPL-2.1-or-later | The only probe that reports HDR mastering metadata (ST 2086, MaxCLL, MaxFALL) and dynamic metadata (HDR10+, Dolby Vision) |

Payload cost and how we avoid it: **mpv plus a minimal ffprobe ship in the base install** (roughly 35–45 MB and 10–15 MB respectively). A **full ffmpeg build is 40–60 MB per platform** and is therefore an on-demand component pack, downloaded only when a feature needs it (casting transcode, bitmap subtitle extraction), verified against a pinned checksum and signature before use, and documented in Settings with its size shown before download. macOS notarization and Windows signing must cover each binary we ship, including the base sidecars.

Obligations, checked per release by `xtask check-notices`:

1. Licence text for the exact binary shipped, inside the bundle.
2. A written offer of corresponding source, on the same distribution channel as the binary.
3. The recorded `configure` line for ffmpeg and the build configuration for mpv, plus a `git diff` for any patch we carry.
4. Credit in the About box and on any download page.
5. No obfuscated binary renames, and no EULA term forbidding reverse engineering.
6. For the GPL mpv: the GPL source offer, and the same obligations applied to it.

## 6. Licence obligations for statically linked dependencies

| Situation | Obligation |
| --- | --- |
| MIT / Apache-2.0 crates and packages (the bulk) | Ship a generated third-party licence list. Apache-2.0 additionally requires including any upstream `NOTICE` file |
| `encoding_rs` | Retain the BSD-3-Clause notice, including its no-endorsement clause |
| `moka` | Attribution under both licence parts |
| `walkdir`, `notify` | Attribution for `walkdir` under Unlicense/MIT; `notify` is CC0-1.0 |
| `lucide-react`, `eslint`-adjacent ISC packages | Attribution; dev-only packages are not redistributed |
| Anything MPL-2.0 | File-level copyleft: publish modifications to MPL-licensed files. **No MPL-2.0 dependency ships today**, because the subtitle crates were replaced with our own parsers, and it is not on the `cargo-deny` allowlist. Adding one requires an explicit allowlist change and an update to this section |
| Anything GPL or LGPL | Must remain outside the process boundary as a separate executable ([adr/0009](./adr/0009-licensing-and-redistribution.md)) |

`cargo-deny` enforces an allowlist of MIT, Apache-2.0, BSD-3-Clause, ISC, Unlicense, CC0-1.0, and Zlib. A new licence stops the build until this document and an ADR are updated.

## 7. Known risks and unverified items

| Item | Status | Handling |
| --- | --- | --- |
| `specta` / `tauri-specta` have no stable 2.x | **Known risk**, only `2.0.0-rc.25` | Exact pin, `SPIKE-9` confirms it end to end in Phase 0, and the fallback is hand-written DTOs with a generation check |
| TypeScript 7, Vite 8, ESLint 10, `@tanstack/react-table` 9 are fresh majors | Known risk | Verify the whole frontend toolchain together as the first Phase 0 frontend task; pin exact versions until CI is green on all three OSes |
| `typescript-eslint` 8.71.1 peer range against TypeScript 7 | **Unverified** | Check the manifest before adopting TypeScript 7; both are needed for `pnpm lint` |
| SOCKS5 coverage of **UDP tracker** traffic in librqbit | **Unverified** | `SPIKE-3` must test it before the VPN-binding feature is described as leak-proof. Until then, the leak check reports rather than guarantees |
| `rust_cast` maintenance (0.21.0, December 2025, 158 stars) | Marginal | Adapter is isolated behind the cast port; DLNA is the primary path for TVs |
| `blurhash` (0.2.3, July 2024) quiet | Low risk | Pure algorithm crate; `SPIKE-4` compares it with thumbhash and a 16 px WebP before committing |
| `keyring` 4.2.0 backend differences on Linux | Known variance | Secret Service with an explicit encrypted-file fallback and a clear warning in the UI |
| GitHub star counts were collected for only 17 repositories (rate limit) | Partial evidence | Adoption decisions therefore rest primarily on release recency and download signal, both of which were captured for every entry |
