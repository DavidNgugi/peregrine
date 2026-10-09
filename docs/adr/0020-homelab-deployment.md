# ADR 0020 — Homelab deployment: what runs on a home server, and what we refuse to put in a container

- **Status:** Accepted as **post-1.0 roadmap** (Phase 9). Not in the 1.0 figures. The browser console in §3 is **scoped but not yet broken down**, and needs its own record before implementation starts.
- **Date:** 2026-10-09
- **Evidence:** umbrelOS runs a Docker-based app runtime whose packages are a `umbrel-app.yml` manifest plus a `docker-compose.yml`, declare host folder mounts and a `GPU` permission, and are proxied to a browser URL — observed 2026-10-09 in [getumbrel/umbrel-apps](https://github.com/getumbrel/umbrel-apps) (`jellyfin/umbrel-app.yml`, top-level `README.md`), where the published App Store standard requires that "every app package should be understandable from the browser after install" and open to "a web UI, setup flow, login page, or status page … without SSH, CLI access, log scraping, or manual file edits". Unraid installs containers from Community Applications XML templates. NixOS runs glibc-linked, dynamically linked binaries through `nix-ld`, and treats `appimage-run` as the AppImage path ([NixOS wiki: nix-ld](https://wiki.nixos.org/wiki/Nix-ld)). Alpine/musl images cannot run glibc-linked prebuilt binaries, which is why the image base in §2 is glibc ([musl binary incompatibility](https://devbytes.co.in/news/why-are-alpine-docker-images-throwing-errors-for-some-binaries)). Hardware transcode in a container requires passing `/dev/dri` and the host `render` group ([Umbrel GPU acceleration support](https://umbrel.com/support/advanced/gpu-acceleration)). The remaining hosts were checked 2026-10-09: TrueNAS SCALE installs custom applications from a Compose definition through its app API ([`app.create`](https://api.truenas.com/v25.10.0/api_methods_app.create.html)); OpenMediaVault 7 gets Compose from the [omv-extras plugin](https://wiki.omv-extras.org/doku.php?id=omv7:omv7_plugins:docker_compose); CasaOS packages applications as Compose plus [its own metadata](https://raw.githubusercontent.com/xyzbubu/CasaOS-AppStore/refs/heads/main/CONTRIBUTING.md); Fedora Server manages containers with Podman under systemd, where Quadlet is the unit idiom; and Proxmox VE runs OCI workloads as guests — an LXC container or a VM — with unprivileged LXC as the default, which is where bind mounts and UID/GID consistency break ([TrueNAS NFS → Proxmox LXC → Docker](https://github.com/Mxlted/truenas-nfs-proxmox-lxc-docker)).
- **Verification:** `SPIKE-19` (first-run reality on four hosts — umbrelOS, Unraid, a Proxmox unprivileged LXC container, and TrueNAS SCALE: volume ownership, ZFS ACL behaviour, GPU passthrough, state directory across an update, and a clean start with no desktop session)
- **Related:** [0009](./0009-licensing-and-redistribution.md), [0013](./0013-mpv-distribution-and-video-surface.md), [0014](./0014-ffmpeg-build-and-transcode.md), [0018](./0018-media-server.md), [0019](./0019-local-accounts-and-parental-gating.md)

## Context

Homelab operators run a headless Linux server — a NAS, a mini PC, a retired laptop, an Umbrel Home, a Raspberry Pi — and drive it from a browser on another machine. The list of operating systems they actually run is long, and it is worth grouping before designing for it, because the groups need different artifacts:

| Pattern | Platforms | What the host gives us | What we must ship |
| --- | --- | --- | --- |
| **OCI runtime with Compose as the native unit** | Unraid, umbrelOS, TrueNAS SCALE, OpenMediaVault, CasaOS, Debian, Ubuntu Server | A container runtime, a Compose-shaped install path, and usually a web UI | The image plus a Compose file |
| **Podman managed by systemd** | Fedora Server, Fedora CoreOS | Podman and Quadlet, with Cockpit as the management surface | The same image plus a Quadlet or systemd unit |
| **Hypervisor, where we are a guest** | Proxmox VE, XCP-ng | Nothing container-specific: we run inside an LXC container or a VM | A documented guest recipe, not a package |
| **Nix** | NixOS | A declarative module system and no glibc by default | A derivation plus a module |

Most of the list is therefore already served by the container decision in §2 — **the common denominator is OCI, and the majority of these hosts install it through Compose**. Three consequences follow. Podman and systemd are a second supported runtime rather than an afterthought. The hypervisors are a documentation problem, not a packaging problem, and the honest answer for both is "run it as a guest, not on the host". And the platforms whose store is the *normal* install path (umbrelOS, Unraid, CasaOS) need a descriptor, because asking their users to paste a Compose file is the friction the store exists to remove.

The temptation is to answer "we support Linux, so we support that". That answer is wrong for a structural reason.

**Peregrine's entire user interface lives inside the Tauri webview.** [ARCHITECTURE §4](../ARCHITECTURE.md) is explicit: one process tree, a webview renderer, and mpv and ffmpeg as child processes. On a headless host there is no display server, no webview runtime, and no window to embed a video surface into. So the question is not "which distributions do we support" but **"which part of Peregrine can exist without a desktop session, and what does the user actually interact with once it does"**.

Four deployment shapes are possible. Only one of them is cheap, and the cheap one is not sufficient on its own.

| # | Shape | What the user gets | Cost | Verdict |
| --- | --- | --- | --- | --- |
| A | **Headless daemon** (`peregrine-server`, [ADR 0018](./0018-media-server.md) §3) packaged as a container and a service | A running acquisition, library, and serving engine with a config file and a status endpoint; the interface is another client | Low — it reuses the core that v1.4 needs anyway | **Necessary, not sufficient** |
| B | **Browser console** served by the daemon, reusing the React sources | Browse, queue, monitors, and settings in a browser tab | High — a second client of the same core, with its own auth, API, and player story | **Required for the app-store path** |
| C | **Delegate the interface** to Jellyfin and Plex via export and the API subset | A homelab user's existing server browses it, or their TV plays from it | Already committed (Phase 7) | Already covered |
| D | **Desktop app pointed at a remote daemon** — the same GUI, IPC bridged to a server over the LAN | The full interface, on a laptop, against the server's library | Medium, and it reuses the v1.5 client-mode work | **Worth having** |
| E | Ship the Tauri app inside a container with a virtual framebuffer | A GUI nobody can see, or X11 forwarding | Medium, with no user | Refused in §4 |

Two facts from the platform evidence decide the sequencing. First, umbrelOS will not publish an app that requires SSH: the manifest has no terminal, and the store standard wants a browser page after install. Second, an umbrelOS app declares its host mounts and its `GPU` permission in the manifest, so the storage and transcode surfaces must be **declared and configurable**, not assumed from platform conventions.

Four constraints are common to every homelab host, and each one is a landmine on the desktop path.

1. **There is no OS keyring.** NixOS, containers, and minimal NAS distributions frequently have no Secret Service daemon. [ARCHITECTURE §4](../ARCHITECTURE.md) currently puts every API key in the platform keyring, and `keyring` will fail where none exists. A homelab build needs a second, explicitly chosen secret backend — and that backend must be honest about being weaker than a keyring.
2. **Files are owned by somebody else.** Volume mounts arrive with host UIDs and GIDs (`PUID`/`PGID` is the convention these platforms already teach their users), and media directories are often mounted read-only in the name of safety. The daemon must run unprivileged, write only inside its own state directory, and treat a read-only media root as a normal configuration rather than an error.
3. **The network is a bridge, not a NIC.** A container gets a NAT'd interface. Discovery ([ADR 0018](./0018-media-server.md) §1) must bind interfaces by selection and honour an injected port, and torrent seeding needs either host networking or a documented listen-port and forwarding setup. Binding `0.0.0.0` was already refused; in a container that refusal needs a workable alternative rather than a dead end.
4. **State must survive an update.** These platforms update apps by recreating the container. Anything not under an explicitly declared state directory is lost, so the config, the database, the image cache, and the torrent state all have to live behind one path.

## Decision

### 1. Homelab support means the headless daemon, never the desktop app

The homelab target is `peregrine-server`: the same core as the desktop app, with no GUI, no webview, and no mpv surface. It is the mode [ADR 0018](./0018-media-server.md) §3 already commits to for v1.4, and this record adds three things that mode needs before it can be handed to a homelab user: a deployment configuration model, a container and service packaging story, and a secret path that works without a keyring. The target is the four deployment patterns in the Context table, not the nine products listed there — a distribution that ships a container runtime is served by the same artifact whether it calls itself a NAS, a hypervisor, or a server distribution.

The desktop application is **not** shipped as a container, does not gain a virtual framebuffer, and does not become the supported artifact for these platforms. A homelab user who wants the full desktop interface runs the desktop app on a laptop and points it at the daemon (§3).

### 2. One image, one state directory, unprivileged, and glibc

| Concern | Decision |
| --- | --- |
| Base | **glibc**, not Alpine. Our sidecars and prebuilt binaries are dynamically linked against glibc; musl silently breaks them. A `debian-slim`-class base is the boring correct answer |
| Architectures | `linux/amd64` and `linux/arm64`. The ARM target matters because these hosts are frequently ARM boards and Apple-silicon-class machines |
| User | Non-root by default, with `PUID` and `PGID` honoured at startup so a NAS user can match host ownership without `chmod 777`. No `--privileged`, no added capability, no writable root filesystem required |
| Runtime | Docker **and** Podman, including a rootless Podman run, because Fedora Server and Fedora CoreOS manage containers with Podman under systemd. Nothing in the image may assume a Docker socket or the Docker CLI, and a user-namespace remap must not break the state root or the `PUID`/`PGID` behaviour in the row above |
| State | One declared state root (`/config` by default) holding the config, SQLite database and its WAL, the content-addressed image cache, the torrent session state, and the logs. Media roots (`/media`) and download roots (`/downloads`) are separate, configurable, and may be read-only where the deployment only serves |
| Ports | One HTTP port, injected by the platform proxy (`PORT`) and overridable. No baked-in port assumption |
| Lifecycle | The daemon handles `SIGTERM` by draining jobs, flushing the database, and closing the torrent session within the platform's stop timeout; a failed stop must not corrupt state |
| Health | An unauthenticated health endpoint for the platform's own health check, returning liveness and version only — no library data, no paths |
| Generation | The container labels, the Compose file, the Quadlet/systemd unit, the umbrelOS manifest, the Unraid template, the CasaOS descriptor, and the NixOS module default are **generated from one metadata source**, so the packaging surfaces cannot drift apart. A host that has no store (Debian, Ubuntu Server, OpenMediaVault) gets the Compose file and the unit, which is what its users install anyway |
| Registry | `ghcr.io`, published from the same release pipeline and the same tag as the desktop release |

For the published image, [ADR 0009](./0009-licensing-and-redistribution.md)'s obligations are re-checked rather than inherited: the LGPL ffmpeg and mpv builds carry their notices and source offer in the image, and the `libopenh264` patent caveat recorded for casting applies to a redistributed server build too. `xtask check-notices` extends to the image's contents.

### 3. Three ways in, and the browser console is a separate, larger epic

- **v2.0.0 — Deployment (E36).** The daemon, the image, the service units, the NixOS module, and the documentation. The interface is a client the user already has: Jellyfin, Plex, DLNA, or the desktop app.
- **v2.0.0 — Desktop app as a client of a homelab daemon.** The existing GUI gains a "connect to a server" mode that bridges the same IPC surface to a remote daemon over the LAN, reusing the client-mode work in [ADR 0018](./0018-media-server.md) §2E rather than inventing a second client. The desktop app remains the interface for the user's own machine.
- **v2.1.0 — Browser console (E37), scoped but not broken down.** The daemon serves the React UI from the same feature sources, against an HTTP and WebSocket API, with paired-device authentication and the `AccountScope` rules from [ADR 0019](./0019-local-accounts-and-parental-gating.md). It is a second client, and it carries its own decisions: an API surface, a session model, and what playback means in a browser when mpv is not present.

**The App Store submission waits for v2.1.** An umbrelOS app package whose page says "open a terminal and edit a config file" fails the standard the store publishes, and submitting one anyway would be the kind of thing that gets an app delisted. Until the console exists, homelab users install the image themselves through Compose, their NAS's container UI, or the NixOS module, and that path stays supported.

### 4. Configuration is a file plus documented environment variables, and secrets are explicit

The daemon reads one config file under the state root and an environment-variable overlay for the handful of values a container platform must inject (state path, media roots, port, bind interfaces, `PUID`/`PGID`, log level). The precedence is file, then environment, then flag, and it is documented once.

Secrets (`TMDB`, `TVDB`, indexer credentials, and the media-server and console tokens) resolve through an explicitly selected backend:

| Backend | Where it is right | What we say about it |
| --- | --- | --- |
| OS keyring | Desktop and any host that has one | The default, unchanged |
| File | Container, NixOS, minimal NAS distributions | Stored under the state root with owner-only permissions (mode `0600`), never logged, and the setup output says plainly that the file is protected by filesystem permissions rather than a keyring |
| Environment | Compose and Kubernetes-style deployments where the operator injects secrets | Supported for one-shot values; the daemon warns that a variable is visible to anything that can inspect the container's environment |

There is no fourth backend, no plaintext fallback that happens silently, and no auto-detection that picks the weakest option when the keyring is missing. A missing backend is a startup error with an actionable message.

### 5. Discovery and seeding are configured, not guessed

Per [ADR 0018](./0018-media-server.md) §1, the daemon binds only interfaces the user selects and never advertises a VPN or container bridge by accident. In a container that becomes: bind all interfaces *inside* the container, and let the **host** side decide reachability. Because a NAT'd bridge network breaks incoming peer connections, the documented torrent options are host networking on the platforms that allow it (Unraid, Compose with `network_mode: host`) or a published listen port plus a note that forwarding is the operator's job. What we do not do is bundle a UPnP port-mapper, because silently opening a port on someone else's router is not our decision to make.

mDNS advertisement is best-effort inside a container and the platform's own proxy and QR pairing are the dependable paths, exactly as the Wi-Fi-first design already assumes.

### 6. Media roots are presumed read-only until the user says otherwise

The daemon writes only where it owns: the state root, the download root, and the export root. A media root may be mounted read-only, and every feature that today assumes write access to the library — deletion, import, upgrade replacement, sidecar export — must detect the condition at configuration time and say which of its actions are unavailable, rather than failing halfway through a job.

This is a stronger rule than "handle `EROFS`": the Settings surface states the capability of each configured root before any operation depends on it. ZFS hosts raise the same class of problem from the other direction — a dataset's ownership and ACLs, not POSIX modes, decide whether the daemon can write — so the root-capability check reports what the filesystem actually granted rather than what the mode bits claim.

### 7. On a hypervisor we are a guest, never a host package

Proxmox VE and XCP-ng are virtualization platforms. Both can run us, and neither should run us *the way they run their own storage and guests*:

| Host | Supported path | Explicitly not supported |
| --- | --- | --- |
| Proxmox VE | An unprivileged LXC container or a VM, with the media root bind-mounted, `PUID`/`PGID` matched to the host UID/GID map, and ZFS datasets sized before install | Installing the container runtime on the Proxmox host itself, or a privileged LXC container to make mounts "just work" |
| XCP-ng | A Debian or Ubuntu guest VM running the container | Netting into the dom0 management domain |

The unprivileged-LXC default is where bind mounts and UID/GID consistency break for every application of this shape, so the deployment notes in E36 treat it as the normal case and show the mapping rather than presenting a privileged container as the fix.

### 8. What we refuse

| Refused | Reason |
| --- | --- |
| The desktop application in a container, or with a virtual framebuffer | There is no user for a window nobody can see. The daemon plus a real client is the honest shape |
| Mounting the Docker socket into our container | It is a host-root-equivalent capability handed to a media downloader. If a feature needs it, the feature is wrong |
| Running the daemon as root, or requiring `--privileged` | Nothing in this design needs either. GPU access is a device and group pass, not privilege |
| Bundling UPnP/NAT-PMP port mapping | Opening a port on a third party's router without asking is not a default we will ship |
| Shipping a reverse proxy or a TLS terminator in the image | The user already runs one. Our documented answer is their proxy, with the WAN exposure rules from [ADR 0018](./0018-media-server.md) §4 unchanged |
| Kubernetes manifests, Helm charts, and multi-tenant hosting guidance | The image is OCI and runs there; the deployment surface beyond a home server is a different product with a different security model ([ADR 0018](./0018-media-server.md) §6) |
| Making the Tauri desktop build run on NixOS as an AppImage | We document `nix-ld` and `appimage-run` as the community path and support the container and the module as the first-class NixOS artifacts. Chasing AppImage-on-NixOS is a bug we do not own |
| An app-store descriptor for every catalog that has one | Each descriptor is a surface with its own reviewer, release cadence, and rejection reasons. We generate the three whose store *is* the normal install path for that platform — umbrelOS, Unraid, CasaOS — and for everything else the Compose file and the unit are the install path |
| A homelab-specific fork, or any feature available only in the container build | One core, one behaviour. A container-only code path is a bug surface that never runs in the desktop test suite |

## Consequences

Positive:

- Homelab support becomes an **increment on the committed server work**, not a parallel product. The deployable artifact is the headless daemon that [ADR 0018](./0018-media-server.md) already requires for `.disc` placeholders and a 23:00 client connection.
- The platforms on the list are reached through the runtime they already run — a container for most of them, a module for NixOS, a guest for the two hypervisors — and none of them needs a desktop session, a webview, or a display.
- The packaging surfaces are generated from one source, so the labels, the Compose file, the unit, the store descriptors, and the module cannot disagree about a port or a mount.
- Declaring the state directory and the read-only media root explicitly makes a class of data-loss and permission bugs impossible-by-configuration rather than handled-by-exception.

Negative and accepted:

- **A stack of packaging surfaces to keep honest.** The container labels, the Compose file, the Quadlet unit, three store descriptors, and the NixOS module each have their own reviewer and release cadence. This is the ongoing cost of the feature, and the generator in §2 is the mitigation, not a removal of the cost.
- **Two container runtimes, one image.** Docker and Podman disagree about user namespaces, group handling, and how a unit is written. The rootless-Podman case is the stricter one, and a Docker-only assumption will pass CI while failing on Fedora until the matrix covers both.
- **The browser console is the real price.** A homelab user's expectation, and the App Store's standard, is a page they open. That is a second client with its own API, authentication, and playback story — plausibly the largest single item in this phase, and it is deliberately not smuggled into E36.
- **A file-based secret store is weaker than a keyring.** On a host where the state directory is readable, the API keys are readable. We document that rather than implying equivalence.
- **The image redistributes ffmpeg, mpv, and their licensing obligations.** [ADR 0009](./0009-licensing-and-redistribution.md)'s sidecar story was written for per-platform desktop bundles; the image is a new redistribution surface that has to be checked in the same change.
- **Two more architectures to test.** `arm64` doubles the sidecar matrix, and a transcode capability matrix that was written per desktop OS now needs a server variant per host.
- **Support load shifts to environments we do not control**: volume permissions, bridge networking, missing keyrings, and GPU pass-through on hardware we have never seen. `SPIKE-19` exists to find the first-run failures before users do.

## Alternatives rejected

| Alternative | Why rejected |
| --- | --- |
| Declare homelab support without changing anything, on the grounds that Linux is already a supported platform | The desktop build needs a display server and a webview. A NAS has neither, so this is a claim with no artifact behind it |
| Ship only the Jellyfin-compatible API and call the homelab covered | [ADR 0018](./0018-media-server.md) already commits to that, and it serves *other* clients. It leaves acquisition, monitors, and settings with no interface at all on the host |
| Build the browser console first and treat packaging as an afterthought | The console is the expensive half and the image is what makes it reachable. Packaging first also forces the configuration and secret model to be honest early, and that model is what the console would otherwise hard-code |
| Make the browser console a from-scratch application | It duplicates the design system, the query layer, and every screen. Reusing `packages/ui` and the feature sources is the only version of this that stays maintainable |
| Support only Docker Compose and skip the NixOS module | NixOS users asked for by name here do not install by hand-editing Compose; a module is how that audience consumes a service, and the module is small next to the image work |
| Run the AppImage under `nix-ld` on NixOS and document that as support | It works often enough to generate bug reports and never well enough to test. Documented as a community path, not a supported artifact |
| Publish a separate `-homelab` build with its own defaults | Two builds means two behaviours, two bug surfaces, and a container path the desktop tests never execute |
| Mount the Docker socket so the daemon can manage sibling containers | A torrent client with host-root-equivalent access to the daemon that runs every other app on the host is a security incident waiting for a CVE |

## Task breakdown

E36 in [TASKS.md](../TASKS.md) breaks the deployment work into seventeen tasks measured at 56 ideal days, depending on E33 (the headless service) and reusing E32's interface selection and device pairing. E37 (browser console) and the App Store submission stay scoped-but-unbroken-down until E36 lands, because `SPIKE-19` and the first real homelab install will change their shape — the same reasoning that keeps E33–E34 unwritten in [ADR 0018](./0018-media-server.md).

## Verification

- `SPIKE-19` (3 days): install the image on four real hosts — an umbrelOS host and an Unraid host under the platform's own app/container UI, a Proxmox unprivileged LXC container with a bind-mounted media root, and a TrueNAS SCALE host with a ZFS dataset for state — with no SSH after the first `docker` command, and record: whether volume permissions are workable with `PUID`/`PGID` alone, what a ZFS dataset's ACLs actually grant the app user, whether `/dev/dri` GPU access produces a working hardware transcode rather than a silent software fallback, whether the state directory survives an app update and a container recreation, what breaks with no keyring and no desktop session, and whether the daemon stops cleanly inside the platform's stop timeout. Its findings decide E37's shape and the exact defaults in the generated metadata.
- Before v2.0.0 ships, `xtask check-notices` passes against the published image's contents, including the ffmpeg and mpv source offer.
- The NixOS module builds and starts the daemon on a clean NixOS host, with the state directory created, the user and group created, and the firewall opened only for the configured port.
- A four-way configuration test proves the same state root, port, and media root produce the same behaviour through the config file, the environment overlay, Compose, and the NixOS module.
