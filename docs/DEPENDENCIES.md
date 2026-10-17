# Peregrine — Dependencies

Pinned versions, licences, and the obligations that come with redistributing them. Every version below was resolved live against crates.io and npm on **2026-10-08** with `User-Agent: peregrine-plan`; the full evidence, download signals, and unverified items are in [research/dependency-matrix.md](./research/dependency-matrix.md). Stack rationale is in [TECH_STACK.md](./TECH_STACK.md); licence decisions in [adr/0009](./adr/0009-licensing-and-redistribution.md), [adr/0013](./adr/0013-mpv-distribution-and-video-surface.md), and [adr/0014](./adr/0014-ffmpeg-build-and-transcode.md).

## 1. Version policy

| Rule | Detail |
| --- | --- |
| Lockfiles | `Cargo.lock` and `pnpm-lock.yaml` are committed. CI builds with `--locked`. |
| Rust ranges | `"2"`-style ranges for Tauri and its plugins (cargo does not select the published `3.0.0-alpha.x` prereleases for a `"2"` requirement), exact pins for anything whose API we depend on closely. |
| Exact pins | `specta` and `tauri-specta` at `=2.0.0-rc.25`: no stable 2.x exists, so the RC is deliberate and pinned exactly. |
| MSRV | Stable Rust minus two releases, declared in `Cargo.toml` and enforced in CI. |
| Node | Node 22 LTS or newer, pnpm 10 or newer, declared in `engines`. CI and the release workflow both pin the newer end of that range — Node 24 and pnpm 11 — so a local run on the declared floor is supported but is not what the gates execute. |
| Updates | Renovate, grouped weekly. A major needs a PR with a CI pass plus a visual regression run for frontend majors. |
| Sidecars | Exact version plus SHA-256 per OS, upgraded by a scheduled PR that runs the media fixture suite. The mechanism that consumes these pins is [adr/0022](./adr/0022-component-packs-and-sidecar-verification.md); until PG-E26-01 lands them, `sidecars.toml` records the `pinned-per-os` rule instead of a version. |
| New dependency | Needs a stated reason in the PR and a licence check against the `cargo-deny` allowlist. |

## 2. Rust runtime dependencies

| Crate | Version | Licence | Purpose |
| --- | --- | --- | --- |
| `tauri` | 2.12.1 | Apache-2.0 OR MIT | Application shell, IPC, window and menu integration |
| `tauri-build` | 2.7.1 | Apache-2.0 OR MIT | Build script and capability processing |
| `tauri-plugin-{updater, store, window-state, single-instance, os, notification, deep-link, shell, opener, dialog, fs, http, log, global-shortcut, clipboard-manager, process, autostart}` | 2.4–2.13 | Apache-2.0 OR MIT | Platform integration; each used for exactly one documented purpose ([ARCHITECTURE](./ARCHITECTURE.md) §17) |
| `specta` + `tauri-specta` | **=2.0.0-rc.25** | MIT | Rust-to-TypeScript type export for the IPC contract |
| `specta-typescript` | **=0.0.12** | MIT | The TypeScript exporter `tauri-specta`'s `typescript` feature already pulls in at this exact version; named directly because `peregrine-ipc`'s export entry point constructs `Typescript` rather than reaching through a re-export |
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

This table is the licence review and the plan, including packages nothing installs
yet. The list of packages actually bundled into the compiled frontend is derived
from `pnpm-lock.yaml` instead of being copied from here, and it is in
[`THIRD_PARTY.md`](../THIRD_PARTY.md) §Frontend packages, gate-enforced by
`cargo xtask check-notices` (PG-E0-18).

| Package | Version | Licence | Purpose |
| --- | --- | --- | --- |
| `react`, `react-dom` | 19.3.0 | MIT | UI runtime |
| `typescript` | 7.0.2 | Apache-2.0 | Types. **New major (the native port)** — installed as `"@typescript/native": "npm:typescript@7.0.2"` and resolved in `pnpm-lock.yaml` as `typescript@7.0.2`. This is what `.bin/tsc` runs, so `pnpm typecheck` is TypeScript 7. Dev-only, never redistributed; the alias is explained in §7 |
| `@typescript/typescript6` | 6.0.2 | Apache-2.0 | The TypeScript 6 Compiler API `typescript-eslint` parses with, installed under the `typescript` alias. Its own `npm:typescript@^6` dependency is pinned by the `overrides` entry in `pnpm-workspace.yaml`, so the API it exposes is `typescript@6.0.3`. Dev-only, never redistributed; see §7 |
| `@eslint/js`, `globals` | 10.0.1, 17.13.0 | MIT | ESLint ships no rule preset, so `@eslint/js` is the base set `eslint.config.js` extends; `globals` supplies the browser and Node global definitions flat config no longer implies. Dev-only, never redistributed. Added with `PG-E0-09` |
| `@types/node`, `@types/react`, `@types/react-dom` | 24.19.1, 19.3.0, 19.3.0 | MIT | Types for the Node APIs `vite.config.ts`, `vitest.config.ts`, and the token test read, and for React and React DOM, which ship no types of their own. Dev-only, never redistributed. Added with `PG-E0-09` |
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
| `@tauri-apps/plugin-dialog` | 2.8.2 | Apache-2.0 OR MIT | The native file and folder picker, wrapped by `apps/desktop/src/lib/picker.ts` so no feature names it directly. The Rust half is `tauri-plugin-dialog`, registered and granted exactly `dialog:allow-open` (PG-E1-12) |
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
| `yaml-rust2` | 0.13.0 | MIT OR Apache-2.0 | Parsing `pnpm-lock.yaml` for the frontend half of `check-notices` in `xtask`. Dev-only, never shipped. Added with `PG-E0-18`: a hand-rolled YAML reader in a gate is the same correctness risk `toml` avoids, and the alternative — a `pnpm licenses list --json` generator — both needs `node_modules` and reports auto-installed optional peers (`typescript`, `@types/react`) as production packages, so it cannot draw the dev-only boundary |
| `cargo-deny` | 0.20.2 | MIT OR Apache-2.0 | Licence allowlist, bans, advisories — the primary supply-chain gate |
| `cargo-machete` | 0.9.2 | MIT | Unused-dependency gate for the Cargo workspace (`cargo xtask check-dead-deps`). CI-only, never shipped; verified against crates.io on 2026-10-12 |
| `cargo-audit` | 0.22.2 | Apache-2.0 OR MIT | Secondary advisory check; overlaps `cargo-deny`, run weekly rather than per-commit |
| `osv-scanner` | action `v2.7.0` | Apache-2.0 | The OSV database aggregates RustSec with GHSA and the ecosystem databases, and one scan covers both lockfiles. Run weekly in `audit.yml`, not per-commit, because `cargo-deny` already gates the Rust graph on every push |
| `cargo-llvm-cov` | 0.9.1 | Apache-2.0 OR MIT | Coverage with per-crate thresholds |
| `cargo-cyclonedx` | 0.5.9 | Apache-2.0 | The CycloneDX SBOM the release attaches (ADR 0009 §Decision 7). CI-only, never shipped |
| `minisign` | 0.12 | ISC | Signs the release manifest. CI-only, never shipped; the secret key is a repository secret and the public half is published |

CI also uses two actions, pinned by version and digest in `.github/workflows/release.yml`: `softprops/action-gh-release` (MIT) to publish the release, and the `minisign` release archive itself, which the workflow verifies against a recorded SHA-256 before running it.
| `git-cliff` | 2.14.2 | MIT OR Apache-2.0 | Changelog generation |
| `release-plz` | 0.3.170 | MIT OR Apache-2.0 | Release PRs |
| `vergen` | 10.0.3 | MIT OR Apache-2.0 | Git metadata in the About box and diagnostics |
| `vitest`, `@testing-library/react`, `@testing-library/user-event`, `msw` | 5.0.3, 16.3.3, 14.6.7, 3.0.2 | MIT | Frontend unit tests with a mocked IPC bridge |
| `@playwright/test` | 1.64.0 | Apache-2.0 | E2E flows and frame-time measurement |
| `eslint`, `typescript-eslint`, `prettier` | 10.12.0, 8.71.1, 3.9.9 | MIT | Lint and format. The `typescript-eslint` peer range against TypeScript 7 is resolved in §7 |
| `knip`, `publint`, `size-limit` | 6.40.0, 0.3.25, 14.2.0 | ISC; MIT | Dead code, package hygiene, bundle budgets. Only `knip` is installed; `pnpm check-dead-deps` gates on it (PG-E0-15), and `publint` and `size-limit` are not installed yet |

`toml` and `sha2` were resolved and licence-checked live against crates.io on **2026-10-09** with `User-Agent: peregrine-plan`; both are dev-only and appear in no shipped binary. `yaml-rust2` 0.13.0 (MIT OR Apache-2.0, MSRV 1.85, the stable-API sibling of `saphyr`) was resolved and licence-checked the same way on **2026-10-12**, and it is dev-only too: it is the only crate the gate adds, and `cargo deny check` covers it. Every other version above was resolved on **2026-10-08**.

Only `cargo-dist` (renamed upstream to `dist`) was considered and is **not** adopted: the Tauri bundler, driven by `pnpm tauri build` in `.github/workflows/release.yml`, already produces every artifact we ship, and a second release tool would duplicate it. That workflow does not use `tauri-action`, so nothing here depends on it.

## 5. Bundled sidecars

| Component | Version | Licence | Why this build |
| --- | --- | --- | --- |
| `mpv` | pinned per OS | **GPL-3.0-or-later as shipped** on macOS and Windows; GPL-2.0-or-later for the Linux build we make | The official macOS and Windows builds embed FFmpeg built with `--enable-gpl --enable-version3`, so their source offer names both. An LGPL-only build (`-Dgpl=false`) disables `vo_x11.c`, `vo_xv.c`, and `x11_common.*`, which means no X11 video output at all. Shipped as a separate executable, an aggregate rather than a derivative work ([adr/0013](./adr/0013-mpv-distribution-and-video-surface.md), [adr/0025](./adr/0025-prebuilt-sidecars.md)) |
| `ffmpeg` | pinned per OS | **LGPL-3.0-or-later** (`--enable-gpl` and `--enable-nonfree` absent; `--enable-version3` accepted) | Full remux, subtitle, and AAC encode capability without copyleft. Consequence: no libx264, so casting transcodes with hardware encoders or `libopenh264` ([adr/0014](./adr/0014-ffmpeg-build-and-transcode.md), [adr/0025](./adr/0025-prebuilt-sidecars.md)) |
| `ffprobe` | same build as ffmpeg | LGPL-3.0-or-later | The only probe that reports HDR mastering metadata (ST 2086, MaxCLL, MaxFALL) and dynamic metadata (HDR10+, Dolby Vision) |

Payload cost and how we avoid it: **mpv ships in the base install**, measured on 2026-10-17 at 98 MB unpacked on macOS arm64, 122 MB on macOS Intel, and 101 MB on Windows. **ffprobe also ships in the base install**, and it is not small: the Linux LGPL shared archive is 63.5 MB and leaves about 192 MB on disk, and the Windows static archive that carries ffmpeg and ffprobe is 171.5 MB. A **full ffmpeg build is therefore an on-demand component pack**, downloaded only when a feature needs it (casting transcode, bitmap subtitle extraction), verified against a pinned checksum and signature before use, and documented in Settings with its size shown before download. The pinned artefacts and exact sizes are in [`packaging/sidecars.lock.json`](../packaging/sidecars.lock.json) ([adr/0025](./adr/0025-prebuilt-sidecars.md)). macOS notarization and Windows signing must cover each binary we ship, including the base sidecars.

Obligations, checked per release by `xtask check-notices`. The declarations live in [`sidecars.toml`](../sidecars.toml), one entry per executable with its licence, version pin, source, source offer, and build configuration; the checked artefact is [`THIRD_PARTY.md`](../THIRD_PARTY.md), regenerated from those declarations, `Cargo.lock`, and `pnpm-lock.yaml` by `cargo xtask check-notices --update`:

The release copies the licence text for each shipped binary and the written source
offer into every bundle through `bundle.resources` in
[tauri.conf.json](../apps/desktop/src-tauri/tauri.conf.json), which carries
[`notices/`](../notices/README.md) and [`NOTICE`](../NOTICE). The sidecar licence
texts and the generated attribution file land in that directory as the sidecars
are pinned; the written offer is
[`notices/SOURCE-OFFER.md`](../notices/SOURCE-OFFER.md). A release in which any of
them is missing fails rather than publishing.

1. Licence text for the exact binary shipped, inside the bundle.
2. A written offer of corresponding source, on the same distribution channel as the binary.
3. The recorded `configure` line for ffmpeg and the build configuration for mpv, plus a `git diff` for any patch we carry.
4. Credit in the About box and on any download page.
5. No obfuscated binary renames, and no EULA term forbidding reverse engineering.
6. For the GPL mpv: the GPL source offer, and the same obligations applied to it.

## 6. Licence obligations for statically linked dependencies

| Situation | Obligation |
| --- | --- |
| MIT / Apache-2.0 crates and packages (the bulk) | Ship a generated third-party licence list. Apache-2.0 additionally requires including any upstream `NOTICE` file. The runtime inventory — Rust crates linked into the binary and frontend packages bundled into it — is [`THIRD_PARTY.md`](../THIRD_PARTY.md); the licence texts it does not carry belong to the release's bundled-licence manifest |
| `encoding_rs` | Retain the BSD-3-Clause notice, including its no-endorsement clause |
| `moka` | Attribution under both licence parts |
| `walkdir`, `notify` | Attribution for `walkdir` under Unlicense/MIT; `notify` is CC0-1.0 |
| `lucide-react`, `eslint`-adjacent ISC packages | Attribution; dev-only packages are not redistributed |
| Anything MPL-2.0 | File-level copyleft: publish modifications to MPL-licensed files. **Five MPL-2.0 crates ship today**, all unmodified transitive dependencies of the Tauri webview stack — `cssparser`, `cssparser-macros`, `dtoa-short`, `selectors` (through `dom_query` in `tauri-utils` and `wry`) and `option-ext` (through `dirs` in `tauri`, `tray-icon`, and `wry`). They are accepted, one crate at a time, by [adr/0021](./adr/0021-mpl-2-0-transitive-dependencies.md): we carry no patch against them, we keep their notices in the attribution artefact, and we disclose them in the SBOM. The `cargo-deny` exception is pinned by crate and version, so any *sixth* MPL crate still stops the build |
| Anything GPL or LGPL | Must remain outside the process boundary as a separate executable ([adr/0009](./adr/0009-licensing-and-redistribution.md)) |

`cargo-deny` enforces an allowlist of MIT, Apache-2.0, Apache-2.0 WITH LLVM-exception, BSD-2-Clause, BSD-3-Clause, ISC, Unlicense, CC0-1.0, Zlib, 0BSD, and Unicode-3.0, plus the per-crate MPL-2.0 exceptions above. A new licence stops the build until this document and an ADR are updated.

## 7. Known risks and unverified items

| Item | Status | Handling |
| --- | --- | --- |
| `paste` 1.0.15 — `RUSTSEC-2024-0436`, unmaintained | **Accepted, compile-time only.** Arrives through `specta`, the IPC type pipeline itself, which is pinned at `=2.0.0-rc.25` because no stable 2.x exists | Ignored by exact advisory id in `deny.toml`; a proc-macro is expanded at build time and is not present in the shipped binary. Revisit when stable `specta` drops `paste` |
| `proc-macro-error` 1.0.4 — `RUSTSEC-2024-0370`, unmaintained | **Accepted, compile-time only and Linux-only.** Arrives through `glib-macros` and `gtk3-macros`, which exist solely on the Linux GTK stack | Ignored by exact advisory id in `deny.toml`; substituted nowhere because both are inside Tauri's own stack. A third unmaintained crate still fails the gate |
| `specta` / `tauri-specta` have no stable 2.x | **Confirmed working; still a known risk.** `SPIKE-9` is confirmed in PG-E2-01: the RC is pinned exactly at `=2.0.0-rc.25`, `peregrine-ipc` derives `specta::Type` on `ErrorCode` and `AppError`, the `system` `build_info` command exports end to end, the generated `packages/ipc-client/src/generated/bindings.ts` typechecks under the strict config, and `cargo xtask gen-ipc --check` fails byte-for-byte drift in CI | **Fallback if the RC breaks or is abandoned:** delete the derives and the export pipeline, hand-write the DTOs and the `commands` object in `packages/ipc-client`, and keep `cargo xtask gen-ipc` as a **generation check** that fails when the hand-written types and the committed bindings disagree — never remove the gate, because it is what makes "the Rust types are the source of truth" enforceable rather than aspirational. The pin is exact precisely because a patch release can change generated output and would otherwise turn the drift gate red on an unrelated upgrade |
| TypeScript 7, Vite 8, ESLint 10, `@tanstack/react-table` 9 are fresh majors | TypeScript 7 + Vite 8 + ESLint 10 **verified together in PG-E0-09** (`pnpm -r build`, `pnpm typecheck`, `pnpm lint`, `pnpm test` all green); `@tanstack/react-table` 9 stays unverified because nothing installs it yet | Pin exact versions until CI is green on all three OSes |
| `typescript-eslint` 8.71.1 peer range against TypeScript 7 | **Resolved in PG-E0-09: incompatible.** The declared peer is `typescript >=4.8.4 <6.1.0`, and the package throws `typescript-eslint does not support TS 7.0.` when `require('typescript')` reports 7.x — `typescript@7.0.2` is the native port and its `exports` carry no Compiler API (only `version` and `versionMajorMinor`) | Both compiler generations are installed, dev-only and never redistributed, using the alias from the TypeScript 7 announcement: `"@typescript/native": "npm:typescript@7.0.2"` (what `tsc` runs) and `"typescript": "npm:@typescript/typescript6@6.0.2"` (Apache-2.0 per its registry metadata; the thin wrapper the parser reads, which exposes `typescript@6.0.3` — pinned by the `overrides` entry in `pnpm-workspace.yaml` because the wrapper declares `npm:typescript@^6`). `@typescript/typescript6` has to exist because `typescript-eslint` calls `ts.createProgram`, and the TS 7 package exports no such API. Verified on pnpm 11.10.0 in PG-E0-09: `.bin/tsc --version` reports 7.0.2, `.bin/tsc6 --version` reports 6.0.3, `require('typescript')` is 6.0.3 with `createProgram` defined, and `eslint .` exits 0. Revisit when typescript-eslint supports TS >= 7.1 |
| SOCKS5 coverage of **UDP tracker** traffic in librqbit | **Unverified** | `SPIKE-3` must test it before the VPN-binding feature is described as leak-proof. Until then, the leak check reports rather than guarantees |
| `rust_cast` maintenance (0.21.0, December 2025, 158 stars) | Marginal | Adapter is isolated behind the cast port; DLNA is the primary path for TVs |
| `blurhash` (0.2.3, July 2024) quiet | Low risk | Pure algorithm crate; `SPIKE-4` compares it with thumbhash and a 16 px WebP before committing |
| `keyring` 4.2.0 backend differences on Linux | Known variance | Secret Service with an explicit encrypted-file fallback and a clear warning in the UI |
| GitHub star counts were collected for only 17 repositories (rate limit) | Partial evidence | Adoption decisions therefore rest primarily on release recency and download signal, both of which were captured for every entry |
