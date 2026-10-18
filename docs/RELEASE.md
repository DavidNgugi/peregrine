# Releasing Peregrine

Audience: maintainers cutting a release. This is the procedure behind
`.github/workflows/release.yml`; it does not describe development builds.

## The v1.0 release candidate is unsigned

v1.0 ships first as an unsigned release candidate. That is a policy decision,
not an unfinished signing step. The workflow builds the same four bundles a
signed release would, uploads them, and writes `SHA256SUMS`; it needs no Apple
Developer ID, no notarisation credential, no Windows code-signing certificate,
and no minisign key. Do not add signing secrets to the repository for the RC, do
not commit a key, and do not run signing or notarisation by hand.

The cost is visible to users: macOS Gatekeeper and Windows SmartScreen will warn
on first launch. The install checklist below says how to get past the warning
without weakening the machine's security settings.

Signing is two independent layers, documented under [Producing a signed release
later](#producing-a-signed-release-later): a minisign signature on the release
manifest, which is implemented and opt-in, and platform code signing plus
notarisation, which is not implemented yet.

## Before you tag

Run these from the release commit:

```bash
cargo test -p xtask
cargo xtask release-check --allow-unsigned
cargo xtask check-notices
```

`release-check` verifies:

- `Cargo.toml`'s `[workspace.package] version`, `apps/desktop/src-tauri/tauri.conf.json`,
  and `package.json` carry the same version.
- `packaging/sidecars.lock.json` is present and readable.
- `THIRD_PARTY.md` is current, by running the existing `check-notices` gate.
- The release workflow has the `v*` tag and `workflow_dispatch` triggers, still
  builds the four bundle labels, uploads `bundle-${{ matrix.label }}`, and writes
  `SHA256SUMS`.
- It prints whether signing is configured, from environment-variable presence
  only. It never reads or prints a secret value.

Without `--allow-unsigned`, an unconfigured signing environment is a failure.
The flag is the explicit statement that this release is deliberately unsigned;
it does not weaken any other check.

When a dependency or sidecar declaration changed, update the attribution first
and commit the result:

```bash
cargo xtask check-notices --update
```

Bump the version in all three files before tagging. `release-check` refuses a
mismatch, and the workflow refuses a tag whose version differs from
`tauri.conf.json`.

## Cutting the release candidate

1. Commit the version bump and any regenerated `THIRD_PARTY.md`.
2. Run the three commands above and read the result.
3. Create an annotated tag, for example `v1.0.0-rc.1`, and push it.
4. Optionally rehearse first with **Actions → release → Run workflow**, passing
   the tag-shaped version. A dispatch builds, checksums, and uploads the same
   artefacts but stops before the GitHub release, which is what the dry run is
   for.

A tag matching `v*` publishes the release. An `-rc.` tag is marked as a
pre-release; a plain `v0.x` tag is too, until v1.0 is ready.

The published assets are:

- four bundle archives, one per label in the table below;
- `SHA256SUMS`, covering the four archives and the SBOM files;
- `artifacts.manifest.json`, with each artefact's name, byte size, SHA-256,
  version, and commit, plus the written source offer;
- `CHANGELOG.md`;
- the Rust and Node SBOMs;
- `artifacts.manifest.json.minisig` only when signing is configured.

No signing secret is required for this list to be complete. The signature is an
extra file, not a condition for the release.

## What each bundle contains

| Label | Platform | Installer formats |
| --- | --- | --- |
| `macos-aarch64` | macOS on Apple silicon | DMG |
| `macos-x86_64` | macOS on Intel | DMG |
| `windows-x86_64` | Windows x64 | NSIS installer |
| `linux-x86_64` | Linux x86_64 | deb and AppImage |

Each bundle carries the application, the staged `mpv` and `ffprobe` sidecars
where the platform uses them, `THIRD_PARTY.md`, and the files under
`notices/`, including the source offer. Only macOS has been exercised end to
end; Windows and Linux artefacts are built but not yet run.

## Install and verify

Download the archive for the platform and `SHA256SUMS` into the same directory.
The checksum entries are basenames, so verification works on a flat download
directory.

```bash
# Linux, WSL, or Git Bash
sha256sum -c SHA256SUMS

# macOS
shasum -a 256 -c SHA256SUMS
```

Inspect the archive before extracting it:

```bash
tar -tzf peregrine-v1.0.0-rc.1-macos-aarch64-bundle.tar.gz
```

Then install:

- **macOS:** open the DMG and drag Peregrine to Applications. Gatekeeper blocks
  the unsigned app on first launch: Control-click the app, choose **Open**, and
  confirm. Later launches work normally. Do not disable Gatekeeper system-wide.
- **Windows:** run the NSIS installer. SmartScreen shows **More info → Run
  anyway** for an unsigned installer. Verify the SHA-256 before running it.
- **Linux:** make the AppImage executable and run it, or install the deb with
  `sudo apt install ./peregrine_*.deb`. Linux has no OS-level signing warning.

After launch, confirm the version in the installer or archive name. The bundle
contains `THIRD_PARTY.md` and `notices/SOURCE-OFFER.md`; a bundle missing either
is not the artefact this process built.

If a checksum does not match, stop. Do not install the file. Report the release
tag, platform label, and the expected and actual digest.

A signed manifest, when one exists, is verified with minisign:

```bash
minisign -V -p peregrine-release.pub -m artifacts.manifest.json \
  -x artifacts.manifest.json.minisig
```

The public key has to be distributed separately from the release; the workflow
verifies the signature against the copy in `RELEASE_MINISIGN_PUBLIC_KEY` and
does not upload that key. The unsigned release candidate has no key and no
`.minisig` file.

## Producing a signed release later

### Manifest signing (implemented, opt-in)

The workflow signs `artifacts.manifest.json` only when the repository variable
`RELEASE_SIGNING_CONFIGURED` is the literal string `true`. These are the exact
repository secrets it reads:

| Name | Kind | Contents |
| --- | --- | --- |
| `RELEASE_MINISIGN_SECRET_KEY` | secret | the minisign secret key file, base64-encoded on one line |
| `RELEASE_MINISIGN_PASSWORD` | secret | the passphrase protecting that key |
| `RELEASE_MINISIGN_PUBLIC_KEY` | secret | the public key file's contents |
| `RELEASE_SIGNING_CONFIGURED` | variable | the literal string `true` |

Generate the pair once, offline:

```bash
minisign -G -p peregrine-release.pub -s peregrine-release.key
base64 < peregrine-release.key | tr -d '\n'
```

Store the base64 output as `RELEASE_MINISIGN_SECRET_KEY`, the passphrase as
`RELEASE_MINISIGN_PASSWORD`, and the contents of `peregrine-release.pub` as
`RELEASE_MINISIGN_PUBLIC_KEY`. Delete the local copies after the secrets are
stored, keep the public key for verifiers, and never print a secret value in a
terminal that logs. Publish `peregrine-release.pub` where downloaders can find
it, such as the repository or the release notes; the workflow does not upload
it.

With the variable set to `true`, a missing secret fails the release instead of
publishing an unsigned one. That is the intended behaviour: the variable is the
maintainer's statement that signing is configured, so the workflow can tell an
unset secret from an intentionally unsigned release.

To rehearse locally, export the same four names and run:

```bash
cargo xtask release-check
```

The command reports configured signing without reading a value. Without the
variable, use `--allow-unsigned` for the RC path.

Rotating the key means replacing all three secrets, setting the variable again,
and publishing the new public key. Releases signed with the old key keep the old
key; never reuse a compromised key.

### Platform code signing and notarisation (not implemented)

The manifest signature proves the checksum list was produced by the project. It
does not satisfy Gatekeeper or SmartScreen. Those need platform identities,
which the workflow does not use today:

- **macOS:** a Developer ID Application certificate and `notarytool`
  credentials. Every nested binary in the bundle, including the dylibs inside
  `mpv.app`, must be signed before notarisation (ADR 0025).
- **Windows:** an Authenticode certificate or Azure Trusted Signing account.

Adding either is a separate change with its own workflow steps and its own
secret names. Do not add a secret that no step reads, and do not put a
certificate, a private key, or a notarisation password in the repository.

`PEREGRINE_TMDB_TOKEN` is a build secret, not a signing secret: it supplies a
shared TMDB token for release builds, and a build without it simply asks the
user for a key.

## Failure modes

- **Version mismatch.** `release-check` names the two files that disagree. Fix
  the version in all three sources and commit before tagging; re-tag rather than
  moving a tag someone may already have fetched.
- **Stale attribution.** Run `cargo xtask check-notices --update`, review the
  diff, and commit it.
- **A bundle label or upload disappeared from the workflow.** `release-check`
  fails. Restore the matrix entry or the upload step; a release that builds
  three platforms is not a smaller release.
- **Signing configured but a secret missing.** The release job fails at the
  signing check, by design.
- **Signing absent without the flag.** `release-check` fails and says to pass
  `--allow-unsigned` or configure signing. This is the only decision the flag
  makes.
