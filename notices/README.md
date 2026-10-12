# `notices/`

Everything in this directory is copied into every released bundle by
`bundle.resources` in [tauri.conf.json](../apps/desktop/src-tauri/tauri.conf.json),
and `cargo xtask check-notices` (PG-E0-18) fails a release in which it is missing
or incomplete.

The files live at the repository root rather than under `apps/desktop` because
they are redistribution obligations of the whole product, and because the gate
that checks them, the About box that displays them, and the release manifest that
names them all read the same copy.

| File | Produced by | Why it is here |
| --- | --- | --- |
| `SOURCE-OFFER.md` | hand-written, this task | the written offer of corresponding source that ADR 0013 §Decision 1 and ADR 0009 §Decision 4 require beside GPL `mpv` and LGPL `ffmpeg`/`ffprobe` |
| `mpv-LICENSE.txt` | `check-notices`, PG-E0-18, from the pinned sidecar | the GPL text for the exact `mpv` build shipped |
| `ffprobe-LICENSE.txt` | `check-notices`, PG-E0-18, from the pinned sidecar | the LGPL text for the exact `ffprobe`/`ffmpeg` build shipped |
| `THIRD-PARTY.md` | `check-notices`, PG-E0-18, from `Cargo.lock` and `pnpm-lock.yaml` | attribution for the crates and packages that are statically linked or bundled, including the `encoding_rs` no-endorsement notice and the upstream `NOTICE` files Apache-2.0 requires |

The three generated files are absent until PG-E0-18 lands. That is deliberate:
they are generated from the lockfiles and the pinned sidecars so they cannot
drift, and a hand-written stand-in would be exactly the stale attestation ADR
0009 §Decision 7 exists to prevent. Until they exist, `check-notices` fails, and
the release workflow calls it before it archives anything.
