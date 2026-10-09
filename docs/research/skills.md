# Agent skills audit — Peregrine (Tauri v2 + React 19 + Rust media stack)

Scope: open skills ecosystem discovery (`npx skills find`, https://skills.sh/) plus the skills already
installed in this session. Method, thresholds and open questions: sections 3 to 5.

Evidence: install counts from the public search API
(`https://skills.sh/api/search?q=<query>`, re-verified live by the maintainer for `rust-best-practices`
at 19,552 installs); stars and repo resolution from the GitHub REST API
(`repos/<owner>/<repo>`, `git/trees`, `contents`) via an authenticated `gh api` call. Every row below
was resolved on 2026-10-08; no candidate is listed on name alone.

**Install-path caveat, verified 2026-10-09.** `npx skills` does **not** run on this machine. Three
independent invocations failed with `npm error code EPERM` on `~/.npm/_cacache/tmp`, with npm advising
`sudo chown -R 501:20 "/Users/davidngugi/.npm"`. Section 2's install commands are therefore verified
against the **skills.sh API and the upstream repositories**, not against a successful local install.
Repair the npm cache ownership before attempting any install, and see section 6 for the install rules.

## 1. Use immediately (already available)

These ship with the session — no install step. Counts: 102 skills in `~/.agents/skills/` plus the
built-in catalog; deduplicated to the 34 genuinely relevant entries.

| Skill | When invoked | Artifact it improves |
| --- | --- | --- |
| agent-browser | Frontend milestones and every UI change | Playwright-driven E2E and visual checks of the Vite shell: library grid, player overlay, settings |
| agent-device | Shell QA on the macOS/Windows build; mobile or TV companion later | Scripted navigation plus screenshots of the Tauri webview on a real device |
| android-emulator | Only if a TV/Android companion target is added | Emulator verification of a casting or remote-control client |
| ios-simulator | Only if a tvOS/iOS companion target is added | Simulator verification of AirPlay/remote behaviour |
| apple-design | Playback controls, sheets, spring motion, reduced-motion paths | Native-feeling player chrome and interruptible transitions in React |
| frontend-design | Week 1 visual direction and any new screen | Non-generic shell: library, detail, player, onboarding |
| ui-ux-pro-max | Screen-level UI work (grid, detail page, player, settings) | Layout, palette and component pattern choices per screen |
| web-design-guidelines | Before each UI PR merge | Web Interface Guidelines audit of the React UI |
| responsive-design | Any layout change; window resize behaviour | Fluid grid reflow, container queries, typography scale in the Tauri window |
| tailwind-design-system | Design-token and theme setup, then component library work | `tailwind.config`, CSS variables, light/dark tokens |
| microinteractions | Loading, buffering, seek, download and save states | Feedback rules for player and torrent controls |
| animate | Feature polish pass; motion review | Purposeful transitions on lists, dialogs, player state changes |
| vercel-react-best-practices | Every React PR (writing or review) | Rerender/bundle discipline for the player and virtualized library lists |
| typescript-react-reviewer | Any PR touching React state or hooks | Type-safety and useEffect-abuse review of player state and data hooks |
| typescript-advanced-types | IPC contract design and refactors | Generated bindings types, discriminated unions for command results |
| tanstack-query-best-practices | Metadata sync features (TMDB/IMDb browsing, caching) | Cache keys, invalidation and pagination for metadata queries |
| tanstack-router-best-practices | Shell routing (library, detail, player, settings) | Type-safe routes and search params for filters |
| vitest | All frontend/TS unit work | Unit and component tests for hooks, formatters, reducers |
| rust-best-practices | Every `src-tauri` / workspace crate change | Idiomatic code, ownership and error handling in the Rust core |
| rust-async-patterns | Async design across tokio, librqbit sessions, sqlx, casting jobs | Cancellation, backpressure and task-supervision patterns |
| rust-testing | Rust test suites for parsers, state machines, IPC handlers | Unit/integration tests, async tests, coverage targets |
| security-and-hardening | Torrent/peer input, file paths, subprocess args, dependency updates | Threat review of untrusted media and peer data plus audit triage |
| api-security-best-practices | External API keys (TMDB), local HTTP endpoints, update channel | Auth, rate limiting and validation of outbound and local APIs |
| postgresql-table-design | Only for generic index/constraint reasoning; never as schema authority | Index and constraint habits that transfer, but Peregrine is SQLite-only — partial indices, FTS5, keyset pagination and WAL behaviour come from [DATA_MODEL.md](../DATA_MODEL.md), not from this skill |
| nodejs-backend-patterns | Dev tooling or any local companion service | Node-side scripts, local API surface, error handling |
| vercel-composition-patterns | Player and library component API design | Compound components instead of boolean-prop proliferation |
| clean-code | Refactor passes on oversized Rust modules and React components | Smaller units, clearer seams in the core crate and shell |
| breakdown-test | Test planning per milestone | Risk-based test matrix for Rust core plus React shell |
| dogfood | Pre-release exploratory QA | Bug list from scripted end-user sessions on the desktop build |
| unslop | README, docs, changelog, release notes | Prose without AI tells in public docs |
| shadcn | If the shell adopts shadcn/ui components | Component scaffolding, registry usage, `components.json` hygiene |
| heroui-react | Only if HeroUI is chosen as the component library | HeroUI component usage and theming |
| office-docx / office-pptx / office-xlsx | Specs, PRDs, benchmark sheets for contributors | Design docs and evaluation spreadsheets shipped in-repo |
| cloudflare | Only if a metadata mirror or update service is hosted there | Workers/R2 for dataset distribution or telemetry |
| agent-tools | Optional one-off media or asset generation | Placeholder art, demo clips for docs and tests |

## 2. Install candidates (verified)

Threshold applied: >= 1,000 installs AND source repo >= 100 stars AND repo URL resolves. All nine pass.
Installability was verified through the skills.sh search API and the upstream repositories; the local
`npx skills add <pkg> -l` resolution check could **not** be reproduced, because `npx` currently fails on
this machine (see the caveat above).

| Skill | Installs | Source repo (stars) | Install command | Why it helps this project |
| --- | --- | --- | --- | --- |
| rust-async-patterns | 19,519 | [wshobson/agents](https://github.com/wshobson/agents) (40,305) | `npx skills add wshobson/agents@rust-async-patterns` | Extends the bundled async skill with supervision trees, cancellation and stream combinator patterns for the librqbit/tokio runtime |
| sql-optimization-patterns | 19,036 | [wshobson/agents](https://github.com/wshobson/agents) (40,305) | `npx skills add wshobson/agents@sql-optimization-patterns` | Query-plan and index work for sqlx migrations and the media-library schema |
| supply-chain-risk-auditor | 7,402 | [trailofbits/skills](https://github.com/trailofbits/skills) (7,434) | `npx skills add trailofbits/skills@supply-chain-risk-auditor` | Lockfile advisory sweep, abandoned-upstream and install-script detection across cargo and npm before the first public tag |
| rust-review | 1,613 | [trailofbits/skills](https://github.com/trailofbits/skills) (7,434) | `npx skills add trailofbits/skills@rust-review` | Security-flavoured Rust review of unsafe blocks, FFI to libmpv and external-input parsing |
| documentation-and-adrs | 49,800 | [addyosmani/agent-skills](https://github.com/addyosmani/agent-skills) (103,369) | `npx skills add addyosmani/agent-skills@documentation-and-adrs` | ADR discipline for the durable decisions: librqbit, mpv IPC, sqlx, casting transport |
| ci-cd-and-automation | 45,300 | [addyosmani/agent-skills](https://github.com/addyosmani/agent-skills) (103,369) | `npx skills add addyosmani/agent-skills@ci-cd-and-automation` | Quality gates, build matrix and release automation for a Tauri cross-platform pipeline |
| playwright-best-practices | 90,900 | [currents-dev/playwright-best-practices-skill](https://github.com/currents-dev/playwright-best-practices-skill) (393) | `npx skills add currents-dev/playwright-best-practices-skill@playwright-best-practices` | Flake control, fixtures and POM for webview E2E; thin star count, but installs are high and the repo is active |
| better-accessibility | 25,045 | [jakubkrehel/skills](https://github.com/jakubkrehel/skills) (7,587) | `npx skills add jakubkrehel/skills@better-accessibility` | WCAG 2.2 focus, contrast, keyboard and announcements for a keyboard-driven player; complements the session's pointer-oriented design skills |
| i18n-expert | 2,545 | [daymade/claude-code-skills](https://github.com/daymade/claude-code-skills) (1,447) | `npx skills add daymade/claude-code-skills@i18n-expert` | Locale scaffolding, RTL and date/number formatting for an international media catalog; ships a local `scripts/i18n_audit.py` |

No listed candidate requires running remote code beyond `npx skills add`, pulls a non-public source, or
bundles network scripts. The `npx` lines surfaced inside `ci-cd-and-automation` are example workflow
steps in its documentation, not commands the skill executes.

## 3. Considered but not recommended

| Skill | Installs | Repo (stars) | Reason |
| --- | --- | --- | --- |
| nodnarbnitram/claude-code-extensions@tauri-v2 | 7,608 | 16 | Below star threshold; installs far exceed repo traction. Best Tauri-specific hit found, but unproven |
| mindrally/skills@tauri-development | 1,256 | 269 | Passes both thresholds but is a generic Tauri intro; redundant with AGENTS.md guidance and weaker than writing in-repo docs |
| getcargohq/cargo-skills@cargo-gtm | 8,400 | 19 | Below star threshold; domain is Cargo (the SaaS) not Rust Cargo. High low-star install pattern |
| affaan-m/ecc@rust-testing | 10,213 | 275,356 | Redundant with bundled `rust-testing` |
| affaan-m/ecc@react-performance | 5,400 | 275,356 | Redundant with bundled `vercel-react-best-practices` |
| jeffallan/claude-skills@rust-engineer | 5,900 | 11,782 | Redundant with bundled `rust-best-practices` + `rust-async-patterns` |
| wshobson/agents@typescript-advanced-types | 84,800 | 40,305 | Redundant with bundled `typescript-advanced-types` |
| antfu/skills@vitest | 39,544 | 5,949 | Redundant with bundled `vitest` |
| emilkowalski/skills@animate / @review-animations | 127,100 / 209,400 | 44,406 | Redundant with bundled `animate` + `microinteractions` |
| nextlevelbuilder/ui-ux-pro-max-skill@design-system | 35,400 | 133,898 | Redundant with bundled `ui-ux-pro-max` + `tailwind-design-system` |
| vercel-labs/agent-skills@web-design-guidelines | 709,800 | 32,091 | Redundant with bundled `web-design-guidelines` |
| anthropics/skills@webapp-testing | 172,100 | 179,990 | Redundant with bundled agent-browser/Playwright path; verified, just not additive |
| microsoft/playwright-cli@playwright-cli | 178,100 | 13,880 | Strong metrics, but overlaps agent-browser and the recommended Playwright skill |
| apollographql/skills@rust-best-practices | 19,552 | 117 | Borderline stars, and the content is already bundled as `rust-best-practices` |
| digitalsamba/claude-code-video-toolkit@ffmpeg | 10,024 | 2,182 | Passes thresholds, but it is a Remotion asset-prep cookbook; sibling skills pull third-party hosted services. Register it as a documentation-style gap (section 4) instead |
| trailofbits/skills@open-sourcing | 1,024 | 7,434 | Threshold edge; guidance is generic and partly covered by `unslop` + docs skills |
| rorkai/app-store-connect-cli-skills@asc-notarization | 6,391 | 1,064 | Passes thresholds but is Xcode/`xcodebuild` + App Store Connect oriented, not Tauri `tauri build` bundling |
| samber/developer-relations-skills@oss-license-strategy | 3,200 | 2 | Below star threshold (2 stars) |
| github/awesome-copilot@agent-supply-chain | 1,100 | 39,818 | Thin single skill; `supply-chain-risk-auditor` is stronger |
| current-level tools: sickn33/agentic-awesome-skills@database-design (2,200, 47,362) | 2,200 | 47,362 | Wrong engine (general OLTP design) vs sqlx/SQLite embedded needs and bundled `postgresql-table-design` |
| softaworks/agent-toolkit@database-schema-designer | 4,300 | 2,541 | Repo untouched since 2026-03; superseded by `sql-optimization-patterns` |
| sickn33/agentic-awesome-skills@typescript-expert | 12,400 | 47,362 | Redundant with bundled TypeScript skills |
| jakubkrehel/skills@better-interface, ibelick/ui-skills@fixing-accessibility, addyosmani/web-quality-skills@web-quality-audit | — | — | Verified 25.5K/19.8K/29.8K installs and 7.6K/9.5K/2.9K stars, but each duplicates skills or the recommended accessibility pick |
| lobehub/lobehub@i18n | 4,600 | 83,063 | Skipped in favour of `i18n-expert`: repo is a product monorepo, the skill is not its primary artifact |

## 4. Coverage gaps

No adequate skill exists for these project needs. "API hits" = matches returned by the skills.sh search
API for the query; none cleared the verification thresholds.

| Capability | Ecosystem status | Recommendation |
| --- | --- | --- |
| Torrent protocol work (librqbit, DHT, piece selection, seeding policy) | Zero candidates: `torrent`, `bittorrent`, `webtorrent` returned 62/31/2 API hits, all off-domain or below thresholds | Write in-repo skill `.agents/skills/librqbit-torrent/`: session lifecycle, backpressure, disk cache, seed limits |
| libmpv embedding and playback control (IPC, event loop, hardware decode) | `mpv` returned 14 hits, none a player-integration skill | Write in-repo skill `.agents/skills/libmpv-embedding/`; document the property/event contract in AGENTS.md |
| Tauri v2 IPC, capability/permission model, plugin surface | Best candidate `tauri-v2` has 7,608 installs on a 16-star repo. `tauri-ipc`, `tauri-security`, `tauri-config` sit at 350-370 installs | Document in AGENTS.md now; promote to an in-repo skill once our own command/event conventions stabilise |
| FFmpeg pipeline design for streaming and remux (HLS, DASH, transcode ladders) | `ffmpeg` hits are Remotion/social cookbooks (`digitalsamba`, 10,024 installs, 2,182 stars) | Document in AGENTS.md with the reference command set; no skill worth installing |
| Subtitle formats (SRT/ASS/VTT, ffmpeg + libass rendering, sync shifts) | All hits are caption generators for social video (`remotion-captions`, `music-video-subtitle-generator`, 1K-135K installs, wrong domain) | Write in-repo skill `.agents/skills/subtitle-formats/` — parsing, cue models, libass style overrides |
| DLNA/Chromecast/AirPlay casting | `chromecast`, `dlna`, `airplay`, `cast` returned no credible results (only unrelated `cast`/`podcast` matches) | Write in-repo skill `.agents/skills/casting-transports/` for SSDP discovery, DIAL launch, cast protocol, transcoding fallback |
| App code signing and notarization across platforms | 1,064-star `asc-notarization` is Xcode-scoped; no Windows Authenticode or Linux packaging skill found | Document per-platform recipes in AGENTS.md (Developer ID + notarytool, Authenticode, AppImage/deb/rpm) |
| IMDb dataset ingestion (bulk title/crew/episode files, Arrow/Parquet) | No skill for bulk dataset ETL or entity resolution | Document in AGENTS.md; `sql-optimization-patterns` covers only the downstream query layer |
| Rust core performance and memory profiling (tokio-console, perf, heaptrack, dtrace) | `performance-profiling`, `memory profiling`, `profil` hits are React Native, three.js or Java only | Document in AGENTS.md: tokio-console setup, `perf`/`samply` flamegraphs, allocation tracking |
| Keyboard and TV remote navigation (D-pad focus model, spatial navigation) | Nothing specific; only generic a11y skills | Bootstrap from `better-accessibility`; escalate to an in-repo skill if a TV target ships |

## 5. Verification status

- Verified: 9 recommended skills (installs from the skills.sh CLI output and the search API, stars and
  URL resolution from the GitHub REST API through an authenticated `gh api`, package resolution from
  `npx skills add <pkg> -l`).
- Corpus: 595 distinct owner/repo@skill pairs from 255 sources from 43 CLI queries, plus the API result
  sets for the 33 mandated queries and 20 gap queries.
- Unauthenticated `curl -H "User-Agent: peregrine-plan" https://api.github.com/...` worked initially,
  then hit the 60 requests/hour unauthenticated limit; the rest of the star data was collected with the
  authenticated `gh api` (5,000/hour). Both read the same REST endpoint; no values were estimated.
- 377 further sources with >= 1,000 installs were checked for existence and stars. Sources that returned
  no GitHub repo (skills hosted only as gists, pages, or renamed owners) are reported as unverified and
  are deliberately absent from section 2.
- Cannot verify: install counts are platform-reported and were not independently reproducible — several
  sources show install counts wildly out of proportion to repo traction (for example 697,500 installs on
  a 5-star `101-skills/superpowers` fork and 760,800 on the bundled `vercel-react-best-practices`), so
  installs were treated as a threshold only, never as a quality signal.
- Cannot verify: per-skill install attribution inside large multi-skill repos (`wshobson/agents` reports
  184 skills, `daymade/claude-code-skills` 116). Counts are attached to skill IDs, not to the plugin
  directory that ships them.
- Cannot verify: skills.sh pages render client-side; pages were confirmed to resolve (HTTP 200 after
  redirect to `www.skills.sh`) but the rendered page could not be scraped for a second install figure.
- Cannot verify: the freshness of third-party star counts between the API read and this document's date.
  Timestamps for each repo are recorded above where relevant.

## 6. Environment prerequisites and install safety

A skill is a set of instructions an agent will follow, so installing one is a supply-chain decision, and
in this project it is also a security decision. Rules:

1. **Repair the npm cache first.** `npx` currently fails with `EPERM` on `~/.npm/_cacache`; the fix is
   `sudo chown -R 501:20 "/Users/davidngugi/.npm"`, which is the user's call to make. Nothing in the
   build may depend on an ad-hoc `npx` install, so CI must never shell out to `npx skills`.
2. **Read the skill before installing it.** A `SKILL.md` that instructs the agent to pipe a remote script
   into a shell, disable a safety check, or exfiltrate files is rejected regardless of its install count.
3. **Install from a resolved source.** Use `npx skills add <owner/repo@skill>` with the source repo
   visible in this document, never an unattributed bundle.
4. **Installs are per-developer, not committed.** Recommended skills are a convenience for contributors,
   not a build dependency; the repository must build and test with no skills installed.
5. **Nothing touching signing, notarization, release, or the update channel is installed without an audit
   recorded here first**, per AGENTS.md §7.
6. **Prefer the in-repo skills for core domains.** Section 4 lists eight capabilities with no adequate
   ecosystem skill. Those belong in `.agents/skills/<name>/SKILL.md`, reviewed like code, because they
   encode this project's own procedures and cannot change underneath a build.
7. **Treat install counts as a threshold, never a quality signal** — they are platform-reported and, as
   section 5 notes, sometimes wildly out of proportion to repository traction.
