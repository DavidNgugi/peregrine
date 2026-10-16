# ADR 0024 — A managed local indexer manager (Prowlarr) as a component pack

- **Status:** Accepted
- **Date:** 2026-10-16
- **Related:** [0009](./0009-licensing-and-redistribution.md), [0022](./0022-component-packs-and-sidecar-verification.md), [0018](./0018-media-server.md), [0020](./0020-homelab-deployment.md)

## Context

Finding a release needs an indexer, and today that means the user installs and wires an indexer manager by hand. Prowlarr is the de-facto manager for torrent indexers, is **GPL-3.0**, and publishes per-OS artifacts (macOS app bundle, Linux/Windows archives) on GitHub releases.

[ADR 0009](./0009-licensing-and-redistribution.md) permits copyleft only out of process; [ADR 0022](./0022-component-packs-and-sidecar-verification.md) defines component packs and how a downloaded binary is verified. That mechanism is designed but **unbuilt** (`PG-E26-01`). The alternative — implement indexers natively in `peregrine-indexers` (`PG-E12-05`) — is the long-term route but covers far fewer indexers today.

## Decision

1. **Prowlarr ships as an on-demand component pack, never in the base install.** One pinned version per release and one asset per OS/arch, with byte size and SHA-256 compiled into the binary (`docs/adr/0022` §1). Downloaded only on the user's explicit action, verified before extraction, staged to a temporary file and atomically renamed.
2. **It runs out of process on loopback, managed by the app.** The application creates its data directory, starts it with `-nobrowser`, health-checks `GET /ping`, and stops it on exit. It binds loopback only; nothing about it is LAN-reachable.
3. **The application reads Prowlarr's own `config.xml` for the API key and port**, and stores the key in the keyring. It never mints or guesses a key — the key is Prowlarr's to generate.
4. **A user may instead point at their own Prowlarr** (remote, homelab, or the future media server) with a URL and key. The managed instance is a convenience, not a requirement.
5. **Licence obligations are explicit.** `sidecars.toml` declares Prowlarr's GPL-3.0-or-later, `THIRD_PARTY.md` carries the licence text and a written offer of source, and `SIDECAR_LICENCES` gains that licence deliberately (`docs/DEPENDENCIES.md` §5).

## Consequences

Positive: indexing works with no manual install, and the key is obtained automatically; the same pack mechanism later carries ffmpeg's on-demand pack.

Negative and accepted: we distribute a GPL binary and therefore owe a source offer and published build configuration per release; we run a managed HTTP listener that `SECURITY.md` must threat-model; we pin and update per-OS binaries; and we take on the component-pack mechanism's verification work. Minisign signing is owed until a release key exists — SHA-256 pinning compiled into the binary is the floor, and [ADR 0009](./0009-licensing-and-redistribution.md) still owes the project a licence review before 1.0.

## Alternatives

- **Bundle Prowlarr in the installer.** Rejected: larger installs, slower updates, a worse licence posture, and every user pays for a component most may not want.
- **Require the user to install Prowlarr themselves (status quo).** Rejected: this is exactly the friction the decision removes.
- **Native indexers only (`PG-E12-05`).** Kept as the long-term route; it does not preclude the pack, and the pack does not replace it.
