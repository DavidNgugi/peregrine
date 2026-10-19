# ADR 0030 — Usenet acquisition: a native engine, a quarantined archive, and an import boundary

- **Status:** Accepted, post-1.0 (PG-E38). The torrent path is unchanged.
- **Date:** 2026-10-19
- **Evidence:** the torrent-only release model and queue in [acquisition.rs](../../crates/peregrine-app/src/acquisition.rs) and [0006_acquisition.sql](../../crates/peregrine-storage/migrations/0006_acquisition.sql); the pre-write gate in [inspect.rs](../../crates/peregrine-domain/src/inspect.rs); Prowlarr's `protocol` and 301 behaviour in [research/reference-codebases.md](../research/reference-codebases.md); SABnzbd's direct-unpack source; [ADR 0009](./0009-licensing-and-redistribution.md), [ADR 0014](./0014-ffmpeg-build-and-transcode.md), [ADR 0022](./0022-component-packs-and-sidecar-verification.md), [ADR 0025](./0025-prebuilt-sidecars.md); AGENTS R25; SECURITY T5, T36, and T37.
- **Related:** [0005](./0005-librqbit-torrent-engine.md), [0024](./0024-managed-local-indexer.md), [0028](./0028-tv-web-client.md)

## Context

Prowlarr already searches Usenet indexers, but the adapter drops every result whose `protocol` is not `torrent` because every downstream type is torrent-shaped: `IndexerRelease.source` is a magnet, an info hash, or a .torrent URL; `download_job` keys a live transfer by info hash and stores a magnet; the state machine enters `seeding` on completion; the gate reads a torrent's file list before the engine writes a byte. Usenet is not a switch on that path. It is a different transfer engine, a different failure model (missing articles rather than dead peers), and a different post-processing problem (repair and extraction).

The privacy argument is real but narrower than it is usually stated. A Usenet client connects to one paid provider over TLS, so peers and copyright-monitoring swarms never see the address and nothing is uploaded; the provider still sees the account and the articles fetched, the indexer sees the search, and downloading infringing material remains infringement. ADR 0018 §5 already records that a headless host may have no encoder; the analogous Usenet constraint is a paid provider and, for most scene releases, an archive that must be repaired and unpacked. "Safer" must not be sold as "anonymous".

Two shapes were available.

| Shape | What it buys | Why it loses |
| --- | --- | --- |
| A managed SABnzbd or NZBGet sidecar | A mature downloader in weeks, with repair and extraction already solved | A second process, a second credential store, a second queue, an HTTP API to poll, and a second thing to update. It repeats the Prowlarr app-sync seam that [ADR 0024](./0024-managed-local-indexer.md) accepts only because an indexer manager has no library. The queue, import, and safety gate would still be ours, so the seam buys less than it costs |
| Link a RAR/par2 implementation in-process | No child process for extraction | Restricted UnRAR code or a C library inside a `#![forbid(unsafe_code)]` tree; AGENTS R25 keeps copyleft and restricted code at the process boundary. SABnzbd itself shells out to `unrar` |
| Native engine, external tools only at the archive edge | One process, one queue, one gate, one credential store; the mature parts stay separate | More work in this repository: NNTP, NZB, yEnc, article assembly, and the post-processing orchestration |

## Decision

### 1. A native `UsenetEngine`, in a new leaf crate

`peregrine-usenet` owns the port and its adapter: an implicit-TLS NNTP client with `AUTHINFO USER/PASS`, `CAPABILITIES`, `BODY`, and `STAT`; a bounded NZB parser; yEnc decoding with part offsets and CRC32; article assembly into the job's quarantine directory; a connection pool; and statistics. It depends on `peregrine-domain` and runtime libraries, never on Tauri, storage, or the keyring. The app resolves credentials and composes the engine.

`AddNzbRequest` takes a `SafetyReport` exactly as `AddRequest` does: an add that has not passed the gate has no spelling.

### 2. One queue, with the protocol at the edges

`AcquisitionService` remains the one owner of admission, transitions, volume checks, progress, deletion, and notifications. It gains an optional Usenet engine, a protocol-carrying start request, and a handle enum. The branch is confined to the places where a source becomes bytes and a transfer becomes complete; no second queue service, no facade, and no parallel state machine.

The state machine stays one machine. `PayloadComplete` reaches `Seeding` for a torrent and `Completed` for Usenet, because there is nothing to seed. `SeedingSatisfied` is refused for Usenet. A live job is deduplicated by info hash for torrents and by SHA-256 of the NZB for Usenet; a terminal job leaves the live set in both cases.

### 3. The gate gains a second half, and the import becomes the boundary

A torrent's file list is known before a byte is written. An NZB's file names and sizes are known before download, but an archive's real contents are not. The gate therefore runs three times:

1. **NZB entries, before download.** Subjects are treated as untrusted text and reduced to safe basenames; file and total-size caps apply; the extension policy applies. A `.rar`/archive entry raises the archive opt-in, default off.
2. **Archive listing, before extraction.** `7zz l -slt` lists paths, sizes, and attributes; the same `PayloadInspector` judges that listing against the volume's caps and the destination. Extraction starts only if it passes. The archive bytes are in the job's quarantine directory, not the library, and nothing has been opened.
3. **Extracted tree, after extraction.** Every path is resolved again; symlinks, hardlinks, absolute paths, traversal, reserved names, unicode spoofing, nested archives, and an over-cap tree are refused. A refusal deletes the extracted tree and fails the job.

This amends the invariant "inspection precedes any byte written". For Usenet the enforced boundary is **import**, not the first write: the download root is a quarantine, no extracted file is imported or played before step 3, and a refused job leaves nothing extracted. SECURITY.md gains the corresponding threat rows; I4 (a report before `metadata`) still holds because the first report is persisted before the engine is handed the NZB.

### 4. Archives are an opt-in, and never a second prompt

`usenet_setting.allow_archives` is a standing permission, default off, read as `OptIn::Granted` for the archive finding only. A blocked archive job's consent action sets it and re-inspects the stored NZB. Post-extraction inspection always runs with `OptIn::NotGranted`, so a nested archive or a hostile entry is a terminal failure, not a loop of prompts.

### 5. Mature tools at the archive edge, pinned and separate

`7zz` (LGPL-2.1 with the UnRAR restriction) lists and extracts; `par2` (GPL-2.0-or-later) verifies and repairs. Both are separate executables under [ADR 0022](./0022-component-packs-and-sidecar-verification.md)'s pinned, checksum-verified mechanism, shipped in the base install because they are small and every archive job needs them. They are invoked with a fixed argument vector, no shell, a reduced environment, and the archive path after `--`; passwords come only from the NZB meta, live in `secrecy::SecretString`, and are never logged or stored. Passing a password in argv is visible to a same-user process; that local exposure is accepted and recorded, because the alternative is restricted code in-process.

### 6. What is deliberately absent

No posting, no NZB creation, no native NZB indexer (Prowlarr remains the indexer source), no provider account management, no bandwidth scheduler, no seeding, and no play-while-downloading in the first slice. A Usenet post is frequently a RAR set and cannot be played before extraction; a single-file sequential stream is a follow-up, not a promise.

### 7. Multiple servers, primary then backup

A provider is a row with host, port, TLS, username, a keyring key reference, a connection budget, and a priority. The engine asks the primary for a segment and a backup on a missing article or a terminal failure. There are no provider pools.

## Consequences

Positive: one process owns the queue, the gate, the library import, and the delete plan; a Usenet release behaves like a torrent release to every screen; the provider password lives in the keyring; no restricted archive code is linked; a host that cannot repair or extract still downloads and reports exactly what is missing.

Negative and accepted: a new protocol surface to test and maintain; a provider subscription is required; archive extraction is a child process with a password in argv; the first slice cannot stream a RAR set; a Usenet job can be blocked on an archive opt-in that torrents rarely see; `7zz`'s UnRAR restriction and `par2`'s GPL add two sidecars to the notices and source offer.

## Alternatives rejected

| Alternative | Why rejected |
| --- | --- |
| Managed SABnzbd/NZBGet | Second process, second queue, second credential store, second API; the seam buys less than it costs |
| Link `unrar` or a C archive library | Restricted/copyleft code in-process, against R25 and the crate's `forbid(unsafe_code)` posture |
| Pure-Rust RAR and Reed-Solomon | A large, security-sensitive implementation with no mature, trustworthy crate at the pinned date; `par2-rs` changed licence on the release we would have pinned |
| Transcode/remux the archive away | Extraction is the only way to play a RAR set; remux does not unpack |
| Streaming first | Most posts are archives; a stream that stalls at the first volume is worse than an honest post-process phase |

## Verification

- `SPIKE-20` on two real providers: TLS and auth, eight-connection throughput, missing-article rate, a par2 repair, and a scene-set extraction.
- A local NNTP fixture server drives the engine offline; no test reaches a provider.
- The hostile suite: traversal, symlink/hardlink, bomb, nested archive, wrong password, 10 000-file NZB, oversized article, CRC mismatch, cross-origin redirect dropping the API key.
- `cargo xtask check-arch` admits the new leaf and no other edge; `check-determinism` admits no wall clock in it; `check-notices` and `release-check` cover both sidecars.
- The existing torrent suite passes unchanged.
