# Written offer of corresponding source

This file is the written offer ADR [0013](../docs/adr/0013-mpv-distribution-and-video-surface.md)
§Decision 1 and ADR [0009](../docs/adr/0009-licensing-and-redistribution.md) §Decision 4
require for the separate executables Peregrine bundles, and it ships inside the
bundle. It is written once and names the tag and the commit rather than a version,
so the copy inside a 0.4.0 bundle is still true in 1.0.0.

Peregrine is an aggregate of the following programs. Each one is a separate
executable invoked over a documented interface; none of their code is linked into
the Peregrine process.

| Executable | Licence | Corresponding source |
| --- | --- | --- |
| `mpv` | GPL-2.0-or-later | the upstream release named in `notices/mpv-LICENSE.txt`, built from the upstream tree with no local patch |
| `ffmpeg`, `ffprobe` | LGPL-2.1-or-later | one build, configured without `--enable-gpl`, `--enable-nonfree`, and `--enable-version3`; the exact `configure` line is recorded per release |

We offer, to any third party, the complete corresponding source for the exact
binaries shipped in this release, on the same channel as those binaries (the
project's GitHub releases), for at least three years from the date of the release.
The offer is also carried in `artifacts.manifest.json`, which is signed, so the
offer cannot be separated from the checksums it applies to.

## Why the build configuration is part of the offer

The GPL and the LGPL require the scripts used to control compilation and
installation, not just the source tarball. For `ffmpeg` that is the `configure`
line; for `mpv` it is the `meson` invocation and any carried patch. Both are
recorded per release in [DEPENDENCIES.md](../docs/DEPENDENCIES.md) §5, which
`cargo xtask check-notices` gates: a release whose sidecar has no licence file,
no source offer, or no recorded build configuration fails rather than publishing.

## What is still owed by PG-E0-18

The pinned versions, their SHA-256 digests, and the recorded `configure` and
`meson` lines do not exist yet — sidecar acquisition is PG-E0-18 and PG-E26-01.
Until they land there is nothing to name in the table above, which is exactly why
this release workflow calls `check-notices` before it archives anything.
