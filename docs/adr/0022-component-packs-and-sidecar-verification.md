# ADR 0022 — Component packs and sidecar verification

- **Status:** Accepted. Implements the component-pack commitment in [ARCHITECTURE.md](../ARCHITECTURE.md) §12 and the sidecar obligations in [0009](./0009-licensing-and-redistribution.md) §4, [0013](./0013-mpv-distribution-and-video-surface.md) §1, and [0014](./0014-ffmpeg-build-and-transcode.md) §1.
- **Date:** 2026-10-12
- **Related:** [0004](./0004-mpv-out-of-process.md), [0009](./0009-licensing-and-redistribution.md), [0013](./0013-mpv-distribution-and-video-surface.md), [0014](./0014-ffmpeg-build-and-transcode.md)

## Context

Three decisions already fix *what* we ship: mpv as upstream's GPL build, a minimal LGPL `ffprobe`, and a full LGPL `ffmpeg` as an on-demand pack ([0013](./0013-mpv-distribution-and-video-surface.md), [0014](./0014-ffmpeg-build-and-transcode.md)). ARCHITECTURE §12 fixes *how* an optional component must arrive: downloaded only on an explicit user action, verified against a pinned checksum and signature, and never run with elevated privileges.

What is undecided is the mechanism, and it is not a detail. A sidecar is an executable we hand the user's media to. If the verification path can be talked into running an unverified binary, we have built a remote-code-execution channel and dressed it as a download. Four questions have to be answered together:

1. **Where does the expected hash live?** If the application fetches the list of expected hashes at runtime, then the list is the weakest link, not the binary.
2. **What does the signature cover?** Signing an artefact authenticates *that file*, not the decision to run it. An attacker who can substitute both the file and its detached signature still needs the application to accept the pairing.
3. **Where is a sidecar found?** A bare name resolved through `PATH` is how a user ends up executing whichever `mpv` was installed first, with flags we did not choose.
4. **What happens when verification fails?** A crash, a silent fallback to an unverified binary, or a named missing capability.

## Decision

### 1. One committed manifest is the source of truth, and it is compiled into the binary

`sidecars.toml` at the repository root is the source of truth. It already exists for the licence obligations PG-E0-18 made mechanical: one `[[sidecar]]` table per component recording its licence, version pin, source URL, written source offer, why it is separate, and its build configuration. This record extends that file rather than adding a second one. Each `[[sidecar]]` gains per-platform-key (`os` × `arch`) artefact entries carrying the pinned version, the artefact URL, its exact byte size, its SHA-256, and its detached signature. The obligation fields and the artefact fields belong together because they describe the same binary, and a version bump has to move both.

`cargo xtask gen-sidecars` turns the artefact entries into a generated Rust table, and the committed generated file is diff-gated exactly like the IPC bindings (`cargo xtask gen-sidecars --check`); `cargo xtask check-notices` keeps validating the obligation half.

The consequence that matters: **the application never fetches a manifest at runtime.** The expected hashes are part of the binary the user already trusts, and the binary was built from a reviewed commit. A bump of a sidecar version is therefore a reviewable diff, not a network event.

### 2. Signatures are minisign, with the public key embedded in the binary

We reuse the scheme Tauri's updater already uses — minisign — rather than introducing a second signature implementation, a second key format, and a second story about where the private key lives. The public key is embedded at build time, one per release channel. The private key is a repository secret and is never printed; a release that cannot find it fails rather than publishing unsigned artefacts.

Verification requires **both** the SHA-256 and the signature: the hash is what the binary pins and the signature is what proves the artefact came from us, so substituting either one alone is not enough.

### 3. Verification is staged, and a partially written file is never runnable

Downloading writes to a temporary file inside the component directory, which is on the same filesystem as the final path so the last step can be an atomic rename. In order:

1. stream to `<component>.part`, counting bytes and hashing as they arrive;
2. reject on the first byte past the declared size, so an oversized response cannot fill the disk;
3. compare the byte count with the manifest;
4. compare the SHA-256 with the manifest;
5. verify the detached minisign signature against the embedded public key;
6. `rename` into place.

A failure at any step deletes the temporary file, leaves any previously verified version untouched, and returns a typed error naming the step that failed. Nothing in the application can spawn a path that has not completed step 6, and the check is a capability the spawn path must hold (a `VerifiedSidecar` token, mirroring `SafePath`), not a convention.

### 4. Resolution order is explicit, and `PATH` is not in it

In order: an explicit user override configured in Settings; the managed component directory; the directory beside the application binary. A bare name is never resolved through `PATH` by default. A user who wants their own build points at a specific file, which is also what [0014](./0014-ffmpeg-build-and-transcode.md) §2 already promises for a user-supplied ffmpeg.

Bundled sidecars are verified too. A tampered file inside the installation is refused by the same path, because the threat is not limited to the network.

### 5. Re-verification happens on change, not on every spawn

Hashing a 60 MB `ffmpeg` before every launch would cost more than the transcode it enables. The install records the verified digest and the file's size and modification time; a spawn checks the size and modification time against that record and re-hashes only when they differ. A file that changes re-enters the full staged verification before it is run again.

### 6. A missing or refused sidecar is a named capability loss

Absence and refusal are surfaced as a missing *capability* — `probe`, `playback`, `transcode`, `subtitle-extract` — with the reason, in Settings and in Diagnostics. The application starts, states which features are unavailable, and disables the actions that need them. It never falls back to an unverified binary and never fails a user's action mid-job with a mystery.

### 7. The `ffmpeg` build configuration is asserted, not assumed

[0014](./0014-ffmpeg-build-and-transcode.md) §2 hangs on the binary not being GPL. Before an `ffmpeg` sidecar is accepted, the recorded `configure` line is checked for `--enable-gpl`, `--enable-nonfree`, and `--enable-version3`, and the shipped binary's own configuration output must agree. A build that disagrees is refused with a licence error rather than used.

## Consequences

Positive: the verification story has one source of truth, one hash function, one signature scheme, and one place where a version bump is reviewed; a compromised mirror cannot substitute a binary; a tampered installation is detected; and the failure mode users see is a named missing feature rather than a crash or, worse, a silent success.

Negative and accepted:

- A sidecar version bump is a two-file diff (the manifest and the generated table) and needs a fresh hash and signature from whoever built the binary. That is deliberate: it is the review gate.
- The download path is more code than `std::fs::write` followed by `chmod +x`, because the atomic-rename and staged-verification invariants have to hold under interruption. The edge cases are the feature.
- Reusing minisign ties component-pack verification to the updater's key management. If the updater is ever replaced, this record is revisited — an acceptable coupling, because two independent signing stories is the worse outcome.
- On Windows an executable cannot be renamed over a file that is currently running, so replacing a sidecar while it is in use must fail cleanly and retry after exit. Recorded here because it is a platform behaviour, not a bug.

## Alternatives rejected

| Alternative | Why rejected |
| --- | --- |
| Fetch the manifest at runtime over HTTPS | Trust-on-first-use for the one document that decides what is allowed to run. TLS authenticates the host, not our intent, and a compromised or merely changed CDN response becomes executable |
| Verify only a detached signature over the artefact | Authenticates the file, not the choice to run it. Combined with a runtime manifest, an attacker who controls the manifest supplies their own artefact and its own valid signature |
| Rely on HTTPS alone | No integrity against a compromised mirror, a mis-issued certificate, or a local proxy the user's machine trusts. For an executable we spawn with the user's media, that is not a control |
| Pin the hash in the manifest but keep the manifest remote and signed | Strictly better than the above, but it buys nothing over compiling the manifest into the binary and adds a runtime failure mode: a network problem becomes an inability to know what is allowed |
| Resolve `mpv` from `PATH` | Runs whichever build was installed first, with flags we did not choose and capabilities we cannot report. It also makes "hardware decode detected" a lie, because the detection would describe a different binary than the one launched |
| Re-hash before every spawn | 60 MB per launch for `ffmpeg`. The size-and-mtime record catches the realistic tampering case at no meaningful cost |
| Bundle the full `ffmpeg` in the base install | Rejected in [0014](./0014-ffmpeg-build-and-transcode.md) on payload: 40–60 MB per platform for a feature most sessions never use |
| Download and run the official upstream installers | No pinned hash, no pinned version, no control over build configuration, and on macOS a `.pkg` wants elevation — which §12 forbids |

## Verification

- `cargo xtask gen-sidecars --check` fails when the generated table and `sidecars.toml` disagree, the same way `gen-ipc --check` does.
- `cargo xtask check-notices` fails when a declared sidecar has no licence, no source offer, no version pin, no source URL, or no recorded build configuration, and when the generated `THIRD_PARTY.md` drifts from `sidecars.toml` and the two lockfiles (PG-E0-18). It validates the *declarations*, not the presence of a licence text inside a bundle; that half belongs to PG-E26-01.
- A fixture suite proves the verifier refuses, one test per rejection: a byte-count mismatch, a hash mismatch, a valid artefact with a signature from the wrong key, a truncated download, an oversized response, and a file whose size changed after verification. Each asserts that no file was renamed into place and that the typed error names the failing step.
- A test asserts that a spawn cannot be constructed from a path that has not been verified, and that a refused sidecar produces a named capability loss rather than a panic.
- A CI fixture asserts that a sidecar whose configuration output contains `--enable-gpl` is refused (PG-E26-01).
