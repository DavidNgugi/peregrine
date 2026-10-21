# Peregrine on Samsung Tizen

A sideloaded Tizen web app that opens the paired LAN client ([ADR 0028](../../docs/adr/0028-tv-web-client.md)) on a Samsung television. It is a shell, not a second client: it asks for the Peregrine address once, remembers it, and navigates top-level to the page the desktop already serves. The cookie, the API, the images and HLS are the listener's own.

The distribution decision is [ADR 0034](../../docs/adr/0034-tizen-client-distribution.md).

## What it is not

- Not a Home takeover, a launcher, or a debloat tool. It does not touch Home routing, packages, vconf, or boot.
- Not a store app. It is signed for one television at a time with that set's DUID.
- Not a server. The desktop must be running with **Settings → Web access** on, on the same LAN. If the desktop sleeps or quits, the television stops.
- Not a second UI. Every screen after the setup page is the existing web client.

## Requirements

- A Samsung TV on Tizen 6.0 or newer. The reference set is a 2021 `UA65AU8000UXKE` (Tizen 6.0, Chromium 76).
- Tizen Studio with the TV extension and the CLI (`tizen`, `sdb`).
- A Samsung certificate profile: an author certificate and a **distributor** certificate carrying the TV's DUID. See [Tizen Studio's certificate guide](https://developer.samsung.com/smarttv/develop/extension-libraries/web-cli/certificate-manager.html).
- Developer mode on the TV, with **Host PC IP** set to this computer's LAN address. `sdb connect` is refused when the two disagree.
- The desktop on the same LAN, Web access enabled, and an address the TV can reach.

## Certificate and build

The certificate is per television and never in this repository. In Tizen Studio, open **Tools → Certificate Manager**, add a **Samsung → TV** profile, create the author certificate, then create the distributor certificate with privilege **Public** and the TV's DUID (TV menu: **Support → Contact Samsung → Unique Device ID**).

The CLI can do the same when the certificates already exist:

```bash
tizen security-profiles add -n my-tv-cert \
  -a ~/SamsungCertificate/my-tv-cert/author.p12 -p <author-password> \
  -d ~/SamsungCertificate/my-tv-cert/distributor.p12 -q <distributor-password>
```

Build and sign from the repository root:

```bash
PROFILE=my-tv-cert apps/tizen/tools/package.sh /tmp/peregrine-tv
```

The script stages only the runtime files and calls `tizen package -t wgt -s …`, so the output is `/tmp/peregrine-tv/Peregrine.wgt`. The output path must not contain a space; the install step passes it on a command line.

To do it by hand:

```bash
tizen package -t wgt -s my-tv-cert -o /tmp/peregrine-tv -- apps/tizen
```

That packages the tests and README too. Use the script when it matters.

## Install and run

```bash
sdb connect <TV_IP>:26101
sdb devices
tizen install -n /tmp/peregrine-tv/Peregrine.wgt
tizen run -p peregrine1.Peregrine
```

On this TV family `sdb install` only pushes the file and exits 0; `tizen install` performs the install and reports the platform's error. The app then appears in the TV's app row as **Peregrine**.

If `sdb connect` is refused, the TV's developer **Host PC IP** does not match this computer's address. Fix that on the TV, not here.

## First run

1. The TV shows the setup page. Enter the address the desktop shows in **Settings → Web access**, for example `http://192.168.1.50:8642`, and press **Connect**. The on-screen keyboard is the Tizen IME.
2. On the next start the saved address is shown for three seconds with **Press BACK within 3 seconds to change the server**. Press BACK (or OK on **Change server**) to edit it; otherwise the app opens Peregrine.
3. Peregrine asks for a pairing. On the desktop, open **Settings → Web access**, compare the eight digits with the TV, and approve. Leave **Allow downloads** unticked for a read-only television.
4. The pairing cookie is `HttpOnly`, `SameSite=Strict`, and scoped to the listener's origin. The web client never reads it; the browser attaches it to `/api`, images and media.

The address is stored in the app's `localStorage`, not in this repository. A DHCP reservation for the desktop is the reliable way to keep it; a Bonjour name works only if the television can resolve it. A changed address is a new origin and therefore a new pairing.

## Update and uninstall

Rebuild and run `tizen install` again; the same author certificate is required, or the TV treats it as a different app. To remove it:

```bash
tizen uninstall -p peregrine1
```

Uninstalling removes the app, its saved address and its cookie. It does not change Home, packages, vconf, or boot.

## Television checklist

Record the model, firmware, Tizen/Chromium, app version, and date with the results. The table is the record the ADR asks for; update it when a set is checked.

Run on 2026-10-21 against `UA65AU8000UXKE` (2021 AU8000, model `21_NIKEL_UHD_BASIC`), app `0.1.0`. Tizen 6.0 / Chromium 76 is the target the reference project measured on this model; the TV's own firmware string was not readable through the REST endpoint.

| Check | How | Result |
| --- | --- | --- |
| Install and launch | `tizen install`, then the app row | **Pass.** Signed `Peregrine.wgt` installed as `peregrine1.Peregrine`; `tizen run` launched it. |
| First run, setup | Enter `http://<desktop>:8642` and Connect | **Partial.** The test build used a build-time address for the TV run; the setup screen itself rendered at 1920×1080 in headless Chrome. On-TV IME entry was not exercised. |
| Pairing with the desktop code | Compare the TV's eight digits with Settings → Web access; approve | **Pass.** The desktop logged a request from the TV's address; approval paired the TV and it showed Home. |
| Cookie survives an app restart | Relaunch; Home appears without a new code | **Pass.** Several reinstall/relaunch cycles (new process ids) produced no new pairing request from the TV. |
| Home, rails, posters | Remote only | **Pass.** The reopened TV confirmed the grid and posters. Two old-engine faults had compounded: Chromium 76 has no `aspect-ratio`, and its flexbox resolved a percentage width on the non-replaced card-art item against an indefinite container, shrinking the art to 80–100 px inside a full-width track; the image's percentage size then followed the shrunken box. Library and Downloads worked because their posters are direct, explicitly sized elements. The Tizen class now stretches the card art (`width:auto; align-self:stretch`), reserves its 2:3 box, and fills the image from its offsets; the Tizen-UA headless check reports card/art/img all 279×418 with the 342 px poster loaded. |
| Ten-foot layout and focus | 1920×1080 and 3840×2160, `html.tizen` | **Pass on the set.** Six viewport-scaled poster columns (279 px at 1920), larger type, a top bar that scrolls instead of covering Browse's search/filters, hover lift and pointer preview removed, entry autofocus (Home hero, Browse search, Title Play/Resume), and one floating focus plate moved with `transform`/`opacity`. Rendered at both sizes in headless Chrome and confirmed after the reopen. Frame rate was not instrumented; the plate is transform/opacity and the per-tile outline is gone. |
| D-pad only: Home, rails, title, seasons, Search | Remote only, no pointer | **Partial.** The physical remote reached a title and started playback on the final build; Home/Browse focus movement was seen earlier. Seasons and Search were not walked end to end. |
| Playback | Play an HLS transcode; pause, resume, progress | **Pass for start and progress, on a user-started HLS episode.** On the final build a user-started episode ran an HLS session with `h264_videotoolbox`, and `watch_progress` reported 18,315 ms of 3,176,107 ms. A direct-route HEVC MKV check also started and reported. Pause/resume and a longer stability run were not observed. |
| Back | Player → title, inner screen → previous, Home → exit | **Partial.** The physical remote's Back returned from Browse (user); the programmable remote's Back did not stop the player's transcode, so the player and root-exit rows are still unverified. The mapping is unit-tested in `apps/web/src/tizen.test.ts`. |
| Exit | Back at Home leaves the app | **Not run.** |
| 30-minute soak | Play for 30 minutes; watch memory and stutter | **Not run.** A short HLS start was the only playback observed. |
| Uninstall | `tizen uninstall -p peregrine1`; Home and apps unchanged | **Not run.** The app was installed and updated, not yet removed. |

What "pass" means for Phase 1: the cookie line and the playback line are observed, not assumed. The cookie line passed; the playback line passed for start and a progress write on a user-started episode, but not yet for pause/resume, Back, or a soak. If those expose a failure the shell cannot fix, stop and write the Phase 1 failure report that [ADR 0034](../../docs/adr/0034-tizen-client-distribution.md) §4 requires before Phase 2 (packaged assets, bearer token, CORS, media auth) starts.

### Local evidence before the set

- `PROFILE=my-tv-cert apps/tizen/tools/package.sh /tmp/peregrine-tv` produced a signed `Peregrine.wgt` containing only the runtime files.
- The setup and pairing screens rendered at 1920×1080 in headless Chrome; the pairing code was legible at 96 px.
- The listener answered `/` with 200, `/api/me` with 401 without a cookie, and a foreign `Host` with 403.
- The built client contains no optional chaining or nullish coalescing (the build target is `chrome76`) and includes the sRGB token fallbacks, the static `dvh`/`clamp` fallbacks, and the explicit Tizen card-art box with the image stretched from its offsets.
- A headless Chrome run with the Tizen user agent at 1920×1080 and 3840×2160 rendered six columns, loaded the rail poster, autofocused Home → Details, and placed the focus plate 4 px around the target with the card's 12 px radius. After the card-art stretch change the same run reports card/art/img all 279×418 (342 px natural poster). This is the DOM/CSS path, not the set's compositor.
- The television run above is the record of what was observed on the set, not a substitute for the remaining rows.

## Television performance

The reference project's measured recipe is followed on the Tizen class, because a full-stage paint on this class of SoC costs 80–200 ms:

- **One focus plate.** The plate is a single promoted element outside React. It moves with `transform` and `opacity`; its border, radius and size are written only when the target changes, and a 150 ms geometry check follows a layout move such as the hero rotation. The per-tile outline is not drawn while the plate is present.
- **No per-tile effects.** Hover lift, hover shadows and the pointer preview are off on Tizen; the title's focus colour change is suppressed. Focus is the only highlight.
- **No chrome repaint.** The top bar scrolls instead of sticking, and its `backdrop-filter` is off.
- **Cheap first paint.** The first visible row is eager-loaded; the card art reserves its own height; the route transition is `transform`/`opacity` only.
- **Layers are bounded.** Only the plate and the route transition are promoted; tiles are not individually promoted.

## Layout

```text
apps/tizen/
  config.xml          package peregrine1.Peregrine, TV profile, Tizen 6.0
  index.html          setup page
  main.js             address resolution, three-second Back window, navigation
  server-url.js       the one URL rule (classic script, tested from source)
  config.js           build-time default address; empty in git
  style.css           1920x1080 setup screen, D-pad focus
  icons/icon.png      the Peregrine app icon
  tools/package.sh    stage the runtime files, sign with a profile
  test/               URL tests (not packaged)
```

## Limits and troubleshooting

| Symptom | Cause |
| --- | --- |
| `sdb connect` fails / TV absent from `sdb devices` | Developer mode off, **Host PC IP** is not this computer, or the TV went to standby. Check the TV's developer settings; `http://<TV_IP>:8001/api/v2/` reports `developerIP`. |
| `install failed[118, -19], reason: Parsing error` | The package id is shorter than ten characters. This app uses `peregrine1`. |
| Install refuses the signature | The distributor certificate does not carry this TV's DUID, or the package was re-zipped by hand. Repackage with `tizen package`. |
| `sdb install` exits 0 but nothing appears | On this TV family `sdb install` only transfers. Use `tizen install`. |
| The .wgt path has a space | `tizen install` cannot take it. Build under `/tmp/peregrine-tv` or another space-free path. |
| Wrong address, black/error page | Relaunch the app and press BACK during the three-second window. The shell was replaced by the page and cannot come back by itself. |
| The TV stops mid-film | The desktop quit, slept, or changed address. The web client warns below 10% battery; the listener is tied to the app. |
| HLS never starts | The set could not play the transcode. This is a Phase 1 failure-report trigger, not a reason to bolt AVPlay into the shell; see ADR 0034 §4. |
