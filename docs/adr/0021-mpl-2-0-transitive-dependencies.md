# ADR 0021 — MPL-2.0 transitively, and a licence exception scoped per crate

- **Status:** Accepted. Refines ADR [0009](./0009-licensing-and-redistribution.md) §2 and the "Anything MPL-2.0" row of [DEPENDENCIES.md](../DEPENDENCIES.md) §6.
- **Date:** 2026-10-12
- **Evidence:** `cargo deny check` (cargo-deny 0.20.2) against the real graph at `868f714`; reverse-dependency walk of `Cargo.lock`
- **Related:** [0001](./0001-desktop-shell-tauri.md), [0009](./0009-licensing-and-redistribution.md), [0013](./0013-mpv-distribution-and-video-surface.md), [0014](./0014-ffmpeg-build-and-transcode.md)

## Context

The licence gate fails on five crates that declare `license = "MPL-2.0"`:

| Crate | Version | Arrives through |
| --- | --- | --- |
| `cssparser` | 0.37.0 | `dom_query` → `tauri-utils`, `wry` |
| `cssparser-macros` | 0.7.1 | `cssparser` |
| `dtoa-short` | 0.3.5 | `cssparser` |
| `selectors` | 0.38.0 | `dom_query` → `tauri-utils`, `wry` |
| `option-ext` | 0.2.0 | `dirs-sys` → `dirs` → `tauri`, `tray-icon`, `wry` |

Two facts make this a decision rather than a chore.

**Every one of them is a transitive dependency of Tauri itself.** `dom_query` is imported by `tauri-utils` and `wry` for HTML and CSS handling; `dirs` is how Tauri, `tray-icon`, and `wry` resolve platform directories. None is a crate we chose, none is imported by our own source, and none can be removed while ADR 0001 keeps Tauri as the shell. This is not a finding we can fix by picking a different library.

**Two records say it should not be there.** ADR 0009 §2 states that no copyleft code is linked into our process, and DEPENDENCIES §6 states that no MPL-2.0 dependency ships and that adding one "requires an explicit allowlist change and an update to this section". Both are true of the intent and no longer true of the graph. AGENTS R12 says a bend in a recorded decision needs a record, so this is it.

What MPL-2.0 actually requires is narrow. It is file-level copyleft: the obligations attach to MPL-licensed files and to distributing a *modified* version of them. We do not modify these crates, we take them at their published versions, and their source is public at those versions. There is no requirement to license our own files, and no requirement to publish anything we do not already publish through Cargo and the SBOM.

## Decision

1. **Accept MPL-2.0 for these five crates, and only as unmodified transitive dependencies of the Tauri shell.** We ship them at their published versions, we carry no patch against them, and we keep their notices in the generated attribution artefact and disclose them in the release SBOM.

2. **Scope the exception per crate and version in `deny.toml`, rather than adding `MPL-2.0` to the global allowlist.** A sixth MPL crate — direct or transitive, whichever direction it arrives from — still fails the gate and needs its own record. This is the whole point: the difference between "we reviewed these five" and "MPL is acceptable here".

3. **ADR 0009 §2 is unchanged for GPL and LGPL.** mpv and ffmpeg remain separate executables ([0013](./0013-mpv-distribution-and-video-surface.md), [0014](./0014-ffmpeg-build-and-transcode.md)). This record narrows nothing there; a weak-copyleft file-level licence on an unmodified library is a different situation from strong copyleft code linked into our binary, and the gate continues to treat them differently.

4. **If a permissive replacement appears for any of these five, take it and delete the corresponding exception.** The exception is a consequence of Tauri's dependency graph, not a preference.

## Consequences

Positive: the licence gate describes the real graph instead of failing on it, so a genuine new licence still stops the build; the obligation we accept is one we can meet without touching our own licensing; and the disclosure is mechanical, because the SBOM and the attribution artefact are already release requirements.

Negative and accepted:

- We ship five MPL-2.0 files inside the binary and must keep their notices accurate. If any of them is ever patched, the relink and source obligations become real and this record has to be revisited before the patch is carried.
- The exception list is pinned by version, so a routine `cargo update` that moves one of them will fail the gate until the record is updated. That is deliberate friction on a licence, not an accident.

## Alternatives rejected

| Alternative | Why rejected |
| --- | --- |
| Add `MPL-2.0` to the global licence allowlist | Permits a direct MPL dependency and every future one without review. The five crates are acceptable because of how they arrive, which a global allowlist cannot express |
| Vendor or reimplement `cssparser` and `selectors` | They are Servo's CSS parser and selector engine. Reimplementing them to stay compatible with `tauri-utils` and `wry` internals is a large, permanent maintenance liability bought to avoid a notice obligation |
| Replace `dirs` for `option-ext` | `dirs` is reached from inside Tauri, `tray-icon`, and `wry`; we cannot substitute their dependency |
| Drop Tauri for a shell with a permissive graph | ADR 0001 chose Tauri for reasons that have nothing to do with this licence, and every alternative webview shell has its own graph |
| Add the five crates to `[bans] deny` and ignore the licence finding | Turns a policy into an exemption list with no record of why, which is what DEPENDENCIES §6 exists to prevent |

## Verification

- `cargo deny check` is a required CI gate (`ci.yml`, job `supply-chain`), and the five exceptions are pinned by crate name and version, so a new MPL crate fails it.
- The supply-chain job also fails when run against a fixture lockfile that contains a known advisory, proving the advisory half of the gate can still fail.
- `xtask check-notices` lists each of the five in the attribution artefact, and the release SBOM carries them (PG-E0-18, PG-E0-16).
- DEPENDENCIES §6 carries the obligation in the same table as every other licence, with a link to this record.
