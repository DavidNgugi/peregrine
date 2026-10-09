# ADR 0009 — Licensing and redistribution boundaries

- **Status:** Accepted, pending review of the verified dependency matrix
- **Date:** 2026-10-09
- **Related:** [0004](./0004-mpv-out-of-process.md), [0005](./0005-librqbit-torrent-engine.md), [0010](./0010-casting-scope.md)

This is an engineering decision record, not legal advice. The project should obtain a licence review before its 1.0 release. Decisions 3 and 4 were corrected on 2026-10-09 after verified research showed that an LGPL-only mpv build has no Linux X11 video output; see [0013](./0013-mpv-distribution-and-video-surface.md) and [0014](./0014-ffmpeg-build-and-transcode.md).

## Context

Peregrine is an open-source desktop application that will ship signed binaries containing third-party media tooling. Two forces collide: permissive licensing keeps the project easy to adopt and embed, and the best media tooling is LGPL or GPL. Getting this wrong is not a bug that can be fixed later — a licence mistake in a distributed binary can force the whole project's licence to change or force a recall.

The [reference codebase research](../research/reference-codebases.md) surfaced a concrete example of the trap: the Peeri repository commits roughly 57 MB of third-party binaries — aria2c under GPL-2.0, ffmpeg under LGPL-2.1-or-later, and yt-dlp under the Unlicense — alongside a bare MIT licence and no notices.

## Decision

1. **Peregrine's own code is dual-licensed MIT OR Apache-2.0.** Permissive, familiar, and unambiguous for downstream packagers.
2. **No copyleft code is linked into our process.** Every GPL or LGPL component runs as a separate executable, invoked over a documented interface.
3. **mpv and ffmpeg are distributed as separate executables** (sidecars) with different build configurations, because they have different constraints. **ffmpeg** is built LGPL-only: no `--enable-gpl`, no `--enable-nonfree`, no `--enable-version3`. **mpv** is shipped as upstream's ordinary GPL build: building it with `-Dgpl=false` would disable `vo_x11.c`, `vo_xv.c`, and `x11_common.*`, which means no X11 video output at all, and upstream states LGPL mode is intended for libmpv rather than the CLI. The exact `configure` line for each shipped binary is recorded in the repository next to the pinned version and checksum.
4. **Each sidecar ships with its own licence text and a written offer of source** pointing at both our build configuration and the upstream source for the exact version, as the applicable licences require (GPL for mpv, LGPL for ffmpeg).
5. **The torrent engine is in-process and permissively licensed.** If a future adapter requires a copyleft engine, it must run out of process under the same rules as mpv.
6. **No third-party binaries are committed to the repository.** They are fetched or built in CI, pinned by version and checksum, and their licences are recorded in `NOTICE`.
7. **A machine-checked dependency policy** (`cargo-deny` licence allowlist plus a per-sidecar manifest) fails the build on a new licence, and the CycloneDX SBOM attached to each release is the published inventory.
8. **No copyrighted content is redistributed.** Artwork is fetched at runtime from the provider and cached locally on the user's machine; fixtures in the repository are generated placeholders or explicitly licensed public-domain media.
9. **The project's legal position on content and indexers is stated plainly in the README**: Peregrine hosts nothing, indexes nothing by default, and ships every built-in indexer adapter disabled.

## Consequences

Positive: downstream distributions and package managers can ship Peregrine without copyleft entanglement; a permissive core keeps the door open for embedding the crates elsewhere; sidecar boundaries also give crash isolation and independent upgrades, so the licence decision coincides with the engineering preference.

Negative and accepted:

- Sidecar distribution obliges us to publish build configurations and maintain a source offer. That is a documentation chore per release, and it is not optional.
- If a strictly better engine or player is copyleft-only, we must either run it out of process or forgo it.
- Our LGPL-only ffmpeg has no libx264 or libx265, so software H.264 encoding is unavailable. Casting therefore prefers hardware encoders and falls back to `libopenh264`; when no encoder exists at all the app refuses with an explanation rather than shipping a GPL binary ([0014](./0014-ffmpeg-build-and-transcode.md)). A user may still point Peregrine at their own ffmpeg build in Settings, on their own responsibility.
- Dual MIT/Apache licensing means contributors must certify provenance; a DCO sign-off is required on every commit.

## Alternatives rejected

| Alternative | Why rejected |
| --- | --- |
| GPL for the whole project | Simplest licence compliance for linking GPL tooling, but it repels commercial packagers, forbids permissive reuse of the core crates, and is a heavier decision than the product requires |
| Link libmpv or libvlc directly and rely on LGPL dynamic linking | Defence-in-depth for our own peace of mind; the child-process boundary is simpler and also better engineering, so we take both wins |
| Commit prebuilt sidecar binaries to the repository | Exactly the Peeri trap: repository bloat, unclear provenance, and invisible licences |
| Ship with no sidecars and require the user to install mpv and ffmpeg | Fragile onboarding; silently breaks playback for users without those packages, and gives the app no control over the build |
| A permissive-only policy that would also exclude any LGPL component | Would rule out ffmpeg entirely and force a from-scratch media stack, which is not a real option |

## Verification

- `cargo-deny` licence allowlist is a required CI gate; adding a dependency with a new licence requires an ADR update.
- Every release attaches an SBOM and a bundled-licences manifest; `xtask check-notices` fails if a sidecar has no licence file or source offer.
- The exact sidecar licences and the verified crates and packages matrix live in [DEPENDENCIES.md](../DEPENDENCIES.md).
