# `notices/`

Everything in this directory is copied into every released bundle by
`bundle.resources` in [tauri.conf.json](../apps/desktop/src-tauri/tauri.conf.json),
as is the generated `THIRD_PARTY.md` at the repository root.
`cargo xtask check-notices` (PG-E0-18) fails when that generated file drifts from
its three inputs. The gate does not itself check that a bundle contains these
files; the bundler does, because a `bundle.resources` entry that matches nothing
fails the build.

The files live at the repository root rather than under `apps/desktop` because
they are redistribution obligations of the whole product, and because the gate
that checks them, the About box that displays them, and the release manifest that
names them all read the same copy.

| File | Produced by | Why it is here |
| --- | --- | --- |
| `SOURCE-OFFER.md` | hand-written, this task | the written offer of corresponding source that ADR 0013 §Decision 1 and ADR 0009 §Decision 4 require beside GPL `mpv` and LGPL `ffmpeg`/`ffprobe` |
| `mpv-LICENSE.txt` | PG-E26-01, from the pinned sidecar | the GPL text for the exact `mpv` build shipped |
| `ffprobe-LICENSE.txt` | PG-E26-01, from the pinned sidecar | the LGPL text for the exact `ffprobe`/`ffmpeg` build shipped |
| `THIRD_PARTY.md` | `cargo xtask check-notices --update` (PG-E0-18), from `sidecars.toml`, `Cargo.lock`, and `pnpm-lock.yaml` | attribution for the crates and packages that are statically linked or bundled, including the `encoding_rs` no-endorsement notice and the upstream `NOTICE` files Apache-2.0 requires |

`THIRD_PARTY.md` is generated and gate-enforced today;
`cargo xtask check-notices` runs in the `gates` job of
[`.github/workflows/ci.yml`](../.github/workflows/ci.yml) and fails when the
artefact drifts from its three inputs, because a hand-written stand-in would be
exactly the stale attestation ADR 0009 §Decision 7 exists to prevent. The two
sidecar licence texts are not written yet: they land with the pinned sidecars in
PG-E26-01. `bundle.resources` in
[tauri.conf.json](../apps/desktop/src-tauri/tauri.conf.json) carries `NOTICE`,
this directory, and the generated `THIRD_PARTY.md` at the repository root, so the
attribution ships even though it is generated outside this directory. The two
licence texts in the table above are the remaining gap in this directory.
