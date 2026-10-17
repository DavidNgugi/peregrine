# Security Policy and Threat Model

Peregrine parses hostile input by design: magnet URIs and `.torrent` metadata written by strangers, media containers from unknown sources, subtitle files, metadata payloads, and device responses on a local network. This document is the security contract for the project. Architecture context is in [docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md) §12; the rules contributors must follow are in [AGENTS.md](./AGENTS.md).

## Reporting a vulnerability

Report privately through GitHub Security Advisories on the repository ("Report a vulnerability" on the Security tab). Do not open a public issue.

| Item | Policy |
| --- | --- |
| First response | Within 7 days |
| Fix or mitigation target | 90 days from acknowledgement, sooner for actively exploited issues |
| Disclosure | Coordinated. We credit reporters unless asked otherwise, and we publish an advisory with the fixed version |
| Scope | The application, its bundled sidecars' configuration and invocation, the update mechanism, and the local HTTP server |
| Out of scope | Vulnerabilities in upstream mpv, ffmpeg, or librqbit that we cannot fix ourselves. We track and update them; report those upstream, and tell us so we can bump the pin |

Supported versions: the latest stable release and the immediate previous minor. Beta builds receive fixes but are not supported configurations.

## Assets

| Asset | Why an attacker wants it |
| --- | --- |
| User files | Arbitrary write or delete outside the intended directories, or exfiltration through a crafted path |
| API keys and Trakt tokens | Credential theft from the keyring, config, logs, or diagnostics bundles |
| Code execution | Running anything on the user's machine. The goal of most attacks below |
| The local HTTP server | Reaching or redirecting media streams, or using the app as a proxy into the machine |
| The update channel | Persistent compromise of every user |
| Privacy | Learning what the user watches, or what is on their disk |
| Availability | Corrupting the library database or filling the disk |

## Trust boundaries

| Boundary | Trust level | Primary controls |
| --- | --- | --- |
| Swarm and torrent metadata | Hostile | Inspection gate before any byte is written; path validation; extension allowlist; caps; no execution; never opened with the system handler |
| Provider HTTP responses | Untrusted | Size caps, content-type checks, validating decode, no HTML rendering of provider strings, image decode through a bounded decoder |
| Media, subtitle, and container files | Hostile | Decoded by out-of-process mpv and ffmpeg; our parsers are fuzzed; subtitle text is treated as data and rendered by libass, never by our UI |
| Local webview | Semi-trusted | Tauri capabilities per window, strict CSP, no `eval`, no remote code in the main window, sandboxed trailer webview with no IPC |
| Loopback HTTP server | Any local process, and any web page via DNS rebinding | Loopback-only bind on an ephemeral port, a per-session token in the request path, `Host` header validation, no listing, `no-store` and `nosniff` on every response, and a socket that dies with the session (ADR 0023, T10) |
| LAN HTTP server (casting) | Any device on the network | Off by default, exists only during a session, interface-bound, separate token, optional PIN, single-file-scoped routes |
| Local network device responses | Untrusted | Length-bounded parsing of SSDP and mDNS responses, no code or path from a device is ever trusted |
| Filesystem | User data | `PathGuard` canonicalization with root allowlists and symlink-escape rejection; the only way to obtain a writable path |
| OS keyring | Trusted | Secrets never in the database, never in config exports, never logged; a redaction layer on all logging |
| Update manifest | Trusted if signed | Signature verification with an embedded public key, downgrade protection, beta channel opt-in |

## Threat catalogue

| # | Threat | Control |
| --- | --- | --- |
| T1 | Path traversal in a torrent file entry (`../../.ssh/authorized_keys`, absolute paths, `..%2f` encodings) | Every entry validated against the resolved save path; relative-only; rejection is a hard block with no override |
| T2 | Executable or script payload disguised as media (`movie.mkv.exe`, double extension, unicode right-to-left override) | Extension allowlist plus content sniffing; executables and scripts block unconditionally; homoglyph and override detection |
| T3 | Symlink or hardlink entry escaping the download root | Symlink entries rejected on all platforms; the write path re-resolves through `PathGuard` and refuses an escape |
| T4 | Archive or disk-image bomb (nested archives, `.dmg`, `.iso`, huge reported size) | Blocked by default; the advanced opt-in warns and records an audit event; pre-flight free-space check with a floor |
| T5 | Argument injection through a hostile filename passed to ffmpeg or mpv (`-i`, `--script`, leading `-`, `;`, backticks) | Sidecars are never invoked through a shell; arguments are passed as an array; paths are canonicalized and prefixed so a leading `-` cannot be parsed as a flag; player flags are fixed and include `--no-config` and `--load-scripts=no` |
| T6 | Malicious subtitle file exploiting a parser or renderer bug | Our converters are fuzzed; rendering happens inside mpv/libass out of process; subtitle text is never injected into our DOM |
| T7 | Malicious image exploiting a decoder | Bounded decode (dimension and pixel caps), the `image` crate kept current, decode on a blocking thread behind a catch, and a discarded result on failure |
| T8 | Provider response containing HTML or script injected into the UI | Provider strings are rendered as text nodes only; no `dangerouslySetInnerHTML` anywhere (lint gate); CSP forbids inline and remote script |
| T9 | Compromised webview calling privileged commands | Capabilities are per window and minimal; the player window and trailer webview cannot call filesystem, shell, or download commands |
| T10 | DNS rebinding or a local process reaching the loopback streaming server | `Host` must name loopback and this listener's port, checked **before** the token; a per-session token is required and is compared byte for byte; the token is a **path segment**, because a `<video>` element cannot set a header — see the note below; routes are scoped to a single file and a path the route does not describe is refused; no listing; `no-store` and `nosniff`; the socket dies with the session |
| T11 | Exposed LAN cast server leaking media to the network | Off by default, explicit enablement with a plain-language warning, interface-bound, session-scoped, optional PIN, rate-limited, and closed on session end |
| T12 | Deep link abuse (`peregrine://magnet/...` dropped by a web page) | Magnet deep links require explicit confirmation showing the inspection report before anything is added |
| T13 | Secret leakage through logs, diagnostics bundles, or crash reports | `Secret` wrapper with no `Debug`/`Display`; a redaction layer that also scrubs key-shaped strings; crash reports opt-in and scrubbed; config export excludes secrets |
| T14 | Update tampering or downgrade attack | Signed manifests verified with an embedded public key; downgrades refused outside the explicit beta channel; the previous bundle is retained for offline rollback |
| T15 | Dependency compromise (typosquat, maintainer takeover) | `cargo-deny` bans and licence allowlist, `cargo-audit`, `osv-scanner`, `pnpm audit`, lockfile-only builds, grouped weekly updates with review, and no new dependency without a stated reason |
| T16 | Server-side request forgery through a user-configured indexer URL | Scheme restricted to HTTP and HTTPS; link-local and cloud metadata ranges blocked; loopback permitted only for the documented Prowlarr and Jackett case; redirects to blocked ranges refused |
| T17 | Disk exhaustion causing data loss | Free-space floor blocks new downloads and pauses active ones rather than failing; image cache trims automatically; SQLite writes are transactional |
| T18 | Database corruption destroying the library | Integrity check on boot, pre-migration backup with rotation, refusing to open a newer schema, and a rebuild path that rescans the library |
| T19 | Deletion of the wrong files | Explicit `DeletionPlan` with enumerated paths and byte counts, blockers for in-use and shared files, trash-first default, typed confirmation for large bulk deletes, and path revalidation between plan and confirm |
| T20 | Privilege escalation on Linux through Flatpak or package permissions | Portal-based file access, minimal static permissions, and a documented review of the manifest at each release |
| T21 | Notification content disclosing viewing history to anyone who sees the lock screen or a shared notification centre | Titles appear only while "show titles" is enabled, adult-flagged titles never appear, and no notification body contains a path, secret, magnet URI, tracker URL, or infohash (invariant I15) |
| T22 | Notification abuse: unprompted permission requests, nagging after denial, or bypassing OS focus modes | Consent is requested in the wizard and at the first relevant action, never at launch; a denial is final until the user revisits settings; quiet hours suppress non-critical events; OS Do Not Disturb is honoured and never bypassed |
| T23 | An unknown or malicious MCP client invoking tools against the library | Off by default; first contact requires an in-app pairing confirmation; clients are named, scoped, and revocable; `read` is the only default scope; revocation takes effect on the next call |
| T24 | **Prompt injection**: attacker-written overviews, release names, or file names steering an agent into spending bandwidth or disk | Every externally sourced string is returned inside a labelled `untrusted` wrapper; tool output can never trigger a tool call; every state-changing action requires a human approval in the app window. The consent gate, not the prompt, is the defence |
| T25 | Library and viewing-history disclosure to a third-party model | Nothing reaches an agent unless the user paired it; `read` scope is explicit and listed per client; every call is logged with its arguments summary; a client can be revoked or the interface disabled at any time |
| T26 | The MCP HTTP transport reaching beyond the machine, or a web page reaching loopback via DNS rebinding | Loopback bind only, never LAN; per-client bearer tokens; `Host` header validated; strict protocol-version consistency; the transport is off unless the user enables it |
| T28 | An unauthenticated device on the LAN browsing or streaming the library | The server is off until the user enables it; every request carries a paired per-device token; devices are listed and revocable; an optional PIN covers shared networks |
| T29 | The listener being reachable from the internet | LAN-only by default with no WAN bind; remote access is documented as a VPN concern, and any externally reachable bind requires an explicit opt-in with a warning |
| T30 | A paired device using the server to read what it should not, or to enumerate the library | Per-device library scoping, no filesystem paths in any response, no delete or settings endpoints, and per-device rate limits |
| T32 | A child switching to an adult account to bypass curation | Switching is gated in both directions by the OS credential where one exists, rate limited with backoff, audited, and has no reset or recovery path that skips it |
| T33 | Content leaking indirectly: search suggestions, counts, "similar titles", continue-watching, notification bodies, casting, or media-server enumeration | The account scope is a parameter of every read in the core; outside the scope, content is indistinguishable from absent; the leak suite (SPIKE-18) is permanent |
| T34 | A child acquiring media despite the restriction | Capability checks live in the acquisition service, not the UI; requests require an adult approval that is recorded; agents and server devices are bound to an account |
| T35 | Unlock brute force, or an app PIN being shoulder-surfed or read from storage | OS authentication is primary; app PIN is a documented fallback; attempts back off exponentially and are audited; the PIN is stored only as a hash |
| T31 | Resource exhaustion: repeated transcode requests pinning the CPU or filling the disk | Transcode is capped per device and globally, hardware encode is preferred, direct play is offered first, and the free-space floor applies as it does to downloads |
| T27 | The agent audit trail itself leaking sensitive arguments | `mcp_call.args_summary` is redacted and size-capped at write time; raw arguments are never persisted; tokens exist only as hashes |

### The loopback token is in the URL, and that is bounded rather than hidden

A `<video>` element cannot set an `Authorization` header, so a header-borne bearer token is not
available to the only client the loopback media server has. The token therefore travels as a path
segment — `GET /media/{token}` — which is the one concession in that design and it is recorded here
rather than left to be discovered (ADR [0023](./docs/adr/0023-in-app-video-surface.md) §2).

The usual reasons a token in a URL is bad do not apply, and each of them is a fact about this build
rather than a hope:

* **No `Referer` leaves.** The document's CSP forbids every remote origin, so there is no third party
  for a referrer to reach, and the client is a media element rather than a page that navigates.
* **No history and no logs.** The URL is never written to the database, never emitted as an event, and
  never logged: `SessionToken` has a redacting `Debug`, no `Display`, and the server's own debug line
  carries the port and nothing else.
* **One file, one socket, one session.** The route serves the single path the session was opened for,
  the listener binds an ephemeral loopback port, and the socket closes when the session ends or the
  value is dropped.
* **The token's job is to stop a web page, not a local process.** A process running as the same user
  can already read the file directly, so a token that stopped it would be defending nothing. What the
  token and the `Host` check actually defeat is the DNS-rebinding case in T10 — a page at `evil.com`
  that resolves to `127.0.0.1`, whose request arrives with `Host: evil.com` and is refused before the
  token is read.

What the token is *not* defended against, stated so nobody assumes otherwise: another local process
that observes the renderer's memory or the URL. That process has the file already.

## Controls by area

| Area | Controls |
| --- | --- |
| Memory safety | `#![forbid(unsafe_code)]` in every crate; no FFI; sidecars are separate processes |
| Input validation | Single validated-constructor pattern per type; provider payloads validated on decode; fuzz targets for the release-name, filename, path, torrent, and subtitle parsers |
| Path safety | `PathGuard` is the only route to a writable path; capability-style `SafePath` token accepted by the filesystem layer |
| Process invocation | No shell; argument arrays only; environment cleared to a known set; fixed player and probe flags |
| Network | HTTPS with rustls, certificate validation on, redirects limited and revalidated, per-host rate limits, no user-agent spoofing. The torrent engine binds to the chosen interface or proxy and **fails closed** — peer and tracker IO stop if the binding disappears rather than falling back to the default route |
| Secrets | OS keyring, in-memory protection, redaction everywhere, never serialized |
| Updates | Signed manifests, staged rollout on beta, rollback retention. Optional component packs (for example the full ffmpeg build) download only on an explicit user action, are checksum- and signature-verified, and never run with elevated privileges |
| Supply chain | Lockfiles, SBOM per release, licence and advisory gates, no committed binaries |
| Telemetry | None by default; crash reporting opt-in, scrubbed, self-hostable, and empty-DSN means no network call at all |
| Auditing | Security-relevant decisions (blocked torrents, overrides, deletions) recorded in the event log and visible in Diagnostics |

## Rejected by design: plugin and extension runtimes

Peregrine ships no plugin runtime, no script loading, and no dynamic library loading, so there is no path by which third-party code enters the application. This is a deliberate rejection of a common pattern, not an unimplemented feature. The counter-example is documented rather than hypothetical: qBittorrent's search plugins are fetched from a GitHub raw URL with no signature, hash, or pinning — only a `.py` extension check — and executed by a system Python interpreter, which means module-level plugin code runs with the user's privileges after an ordinary "check for updates" action. Extension in Peregrine is a reviewed pull request against a trait with a conformance suite ([docs/adr/0011](./docs/adr/0011-extension-model.md)).

## Accepted risks

We name these rather than pretend they do not exist.

| Risk | Why accepted | Compensating measure |
| --- | --- | --- |
| A vulnerability in mpv or ffmpeg affects users of a shipped version | We do not control upstream; a fix requires a release on our side | Sidecars are pinned by version and checksum, upgraded on a scheduled PR that runs the media fixture suite, and the update channel can push a sidecar-only release |
| During an active cast session, any device on the LAN that obtains the token URL can fetch the media | Inherent to serving media to a LAN renderer | Off by default, session-scoped, interface-bound, optional PIN, and a plain-language warning before enabling |
| A user who searches a public indexer takes a legal and safety risk | The project cannot audit third-party indexes, and free public ones vary in what they host | The managed Prowlarr starts with five example indexers that need no account, each removable in Settings; Peregrine's own adapters ship disabled; the project hosts and redistributes nothing; rate-limited endpoints are not polled |
| Deep-link magnets are classified by the name in the URI, not verified content | The inspection report is the verification, and it requires user confirmation | Confirmation dialog shows the inspection result before any download starts |
| **App-level accounts on a shared OS login are a usability boundary, not a security boundary** | The same OS user owns the database, the config, and the keyring, so a determined child with file access can read or reset them. Encrypting the database would not fix it, because the running app under the same user must hold the key | OS-authenticated switching (biometric or Hello, bound to the parent's identity), no bypass or reset path, rate-limited and audited attempts, and the limitation stated once in the parental settings. Separate OS accounts are the documented real fix |
| Local database is unencrypted | Encrypting it would break portable mode, backup, and inspection, and the threat model is a local attacker who already has the user's session | Secrets are never in it; filesystem permissions are the boundary |
| macOS binds a keychain item's access to the code signature of the application that created it, so a rebuilt, re-signed, or reinstalled binary is asked for access again | The binding is the OS's, and it is what stops another application from reading our entry silently | The key is never deleted by this: the item stays in the login keychain under `Peregrine`, a denial is reported as `denied` rather than as an absent key (`CredentialStore`), and Settings says the key is still there and how to grant access. The `secret_ref` pointer in the database is not what the app reads, so wiping the data directory cannot orphan a stored key either. A **debug build** composes `DevFileStore` instead — a 0600 file under the app data directory, with the keyring read once to migrate — so a developer is not asked on every rebuild; a release build always composes the keyring |
| A key entered under one OS login is invisible under another, and on a machine with no Secret Service there is nowhere to store one | The store is per-user by construction, and the explicit file-backed backend does not exist yet | The state is reported as `unavailable` rather than as a missing key, so a user is never told their key was deleted; the backend selection is `PLAN` §Phase 9's "Secrets without a keyring" row and is not yet built |
| `ringbuf` 0.4 through `librqbit-utp` carries RUSTSEC-2026-0293 (a double free when a buffered element's `Drop` panics) | The buffer is `SharedRb<Heap<u8>>`, so every element is a `u8` and cannot panic on drop, and no `librqbit-utp` release accepts `ringbuf` 0.5 yet | Ignored by exact advisory id in `deny.toml`; the dependency is upgraded as soon as upstream allows it |
| No sandbox on the packaged desktop app beyond OS defaults | macOS App Sandbox and Flatpak strict confinement conflict with managing user-chosen media directories | Minimal capabilities, no shell, no remote code, and a documented Flatpak manifest with portals |

## Release security checklist

Every release must satisfy all of these before publishing.

1. `cargo-deny`, `cargo-audit`, `osv-scanner`, `pnpm audit` clean, with any exception documented in the release PR.
2. Fuzz targets run for at least 24 hours since the last release without a crash.
3. No secret appears in any log, diagnostics bundle, or crash report; the redaction test passes.
4. Assertion tests pass: no LAN listener when casting is disabled, no MCP transport listening when agent access is disabled, no network call when crash reporting is unconfigured, no `unsafe` code, no `dangerouslySetInnerHTML`.
5. Update manifest signature verified from the previous release on each supported OS.
6. SBOM, bundled licences, and sidecar build configurations attached to the release.
7. Any new dependency with a new licence or a security-relevant role reviewed and recorded in the ADR set.
8. Deletion, path-guard, and torrent-inspection adversarial suites green.
