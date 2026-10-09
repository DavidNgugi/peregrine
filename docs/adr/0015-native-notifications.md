# ADR 0015 — Native notifications: delivery, sound, and permission honesty

- **Status:** Accepted
- **Date:** 2026-10-09
- **Evidence:** `tauri-plugin-notification` 2.5.1 — `plugins/notification/guest-js/index.ts`, `src/desktop.rs`, `Cargo.toml`; [v2.tauri.app/plugin/notification](https://v2.tauri.app/plugin/notification/). Verified 2026-10-09
- **Verification:** `SPIKE-11` (per-OS sound values, silence behaviour, and delivery when the notification server is absent)
- **Related:** [0002](./0002-rust-core-owns-state.md), [0011](./0011-extension-model.md), [0014](./0014-ffmpeg-build-and-transcode.md)

## Context

The product requirement is explicit: native OS notifications, with sound configurable per event, and the user asked before we notify them about things like a finished download.

Reading the plugin rather than its documentation changed four assumptions.

1. **There is no desktop permission prompt to ask for.** The Rust API's `request_permission()` and `permission_state()` on desktop return `PermissionState::Granted` unconditionally, with the source comment "Desktop applications do not need to ask for this permission". The JavaScript `requestPermission()` instead calls the **web** `Notification.requestPermission()`, and `isPermissionGranted()` consults `window.Notification.permission` first. So the available signal is the webview's, not the OS's, and a `true` is not proof the user will see anything: on Linux there may be no notification daemon at all, and on macOS the app can be switched off in System Settings without the webview knowing.
2. **Grouping and scheduling are ignored on desktop.** "Only the title, body, icon, sound and the actions of the action type are used on desktop; the scheduling and grouping options are ignored." Coalescing a burst of finished downloads is therefore entirely ours to implement, not a `group` parameter.
3. **Click reporting has hard platform limits.** On Windows a click is reported "only while the toast is on screen, not once it moved to the Action Center". On macOS, two or more actions collapse into an "Options" menu, a click on that menu button reports the *first* action, and actions are distinguished **by title**, not id. And notifications shown before any handler or listener exists report nothing at all — so a listener must be registered at startup, and actions are not even displayed when no listener exists.
4. **Sound is a single `notify-rust` `sound_name`**, which means different value shapes per OS: macOS takes a system sound name or an in-bundle file, Linux takes an XDG theme name or a path (delivered as the `sound-name` D-Bus hint, the only hint the plugin sets), Windows takes a `.wav` file path. The `silent` option is documented as **iOS-only**, so desktop silence cannot be requested explicitly.

Two further limits matter: `remove_active`/`remove_all_active` do nothing on macOS and Windows, so a notification cannot be retracted once shown; and Windows notifications only work properly for an installed app, with development builds showing the PowerShell name and icon.

## Decision

### 1. The in-app notification centre is the source of truth; the OS toast is a bonus

Every user-facing event writes a row to the `notification` table and appears in the in-app centre **before** any OS notification is attempted. The OS toast is best-effort delivery of the same event. This is not belt-and-braces: it is required by the verified limits above — the notification server may not exist, macOS may have the app muted, Windows may lose the click, and nothing can be retracted. An event is never discoverable only through a toast.

### 2. Ask the user, and be honest about what the ask can achieve

- The first-run wizard includes a Notifications step that explains what we will notify about, offers per-event control, and asks for consent. Declining is a first-class choice and leaves the in-app centre fully functional.
- On consent, we call `isPermissionGranted()` and, if false, `requestPermission()`. On macOS this is what raises a system prompt; on Windows and Linux it typically resolves without one, and that is fine.
- **We never claim the user will see notifications.** Settings → Notifications carries a *Send test notification* button, the resolved permission state, and a link that opens the OS notification settings for the app, because those settings — not our state — are what actually decide delivery.
- If permission is denied, or a test notification fails to arrive, the UI says so plainly, keeps the in-app centre working, and offers the settings link. We never re-prompt unprompted, and we never nag.

### 3. An explicit event catalogue, each with its own enable and sound

| Event | Default | Sound by default | Delivery notes |
| --- | --- | --- | --- |
| Download completed | on | on | Coalesced per burst (see 4). Click opens the title in the library |
| Download failed | on | on | Includes engine errors and stalls that crossed the retry limit |
| Torrent blocked by the safety gate | on | on | **Critical**: ignores quiet hours and cannot be disabled, because it is a security outcome |
| New episode available for a watchlisted show | on | off | Batched into one digest per day when several land together |
| Disk space low | on | on | **Critical** |
| Update available | on | off | Click opens the update prompt, never installs |
| Subtitle download finished | off | — | Background noise by default |
| Library scan or metadata sync finished | off | — | — |
| Cast session ended | off | — | — |

Defaults are opinionated, not arbitrary: things the user is *waiting for* notify with sound, things that happen *to* the library notify silently, and things that indicate a problem always notify. Each row is a settings entry with its own switch and its own sound switch.

### 4. Coalescing, rate limiting, and suppression are ours

Because the OS ignores grouping on desktop:

- **Dedupe window** per `(event, subject)`: a repeated event within 60 seconds updates the existing centre entry instead of adding one.
- **Burst summary**: more than three completions within two minutes produce one notification ("5 downloads finished") whose click opens the queue, not five toasts.
- **Global rate limit**: at most four toasts per minute, remainder folded into the centre silently.
- **Focus suppression**: if the window is focused and the user is already on the relevant screen (they are watching the queue when a download finishes), no toast is shown — the in-app state already tells them. This is the single biggest difference between a considerate app and an annoying one.
- **Quiet hours**: a user-set window during which non-critical events queue in the centre and are delivered as a digest when it ends. Critical events (security, disk) ignore it.
- **Never bypass OS focus or Do Not Disturb.** Honouring them is the feature.

### 5. Sound policy

- **No bundled audio assets.** macOS uses named system sounds, Linux uses XDG theme names, and Windows uses a path to a system `.wav` resolved from `%SystemRoot%` at runtime with a validated fallback list. This avoids shipping (and licensing) sound files and keeps the installer small.
- **Per-event sound** is a toggle, plus a global "Notification sounds" master switch, plus an optional user-supplied custom sound validated by extension and size per OS (`.wav` on Windows; `.wav`/`.aiff`/`.caf` and `.oga`/`.ogg` elsewhere).
- **"Sound off" is best-effort and documented as such.** Omitting the sound hint is the only lever the plugin offers on desktop, and a Linux notification daemon or the Windows toast may still play its own default. The settings UI states this in one line and links to the OS settings rather than pretending otherwise.
- **The only sound we fully control is our in-app fallback chime**: a short two-tone synthesized WebAudio cue, no asset file, played only when the in-app centre raises an entry and OS notifications are unavailable or the user chose in-app-only. It respects the master switch and the system mute.

### 6. Click routing

- A Rust `on_action` handler is registered at startup, before any notification is sent, because actions are not shown at all without a listener and clicks on early notifications are otherwise lost.
- The `extra` payload carries a deep link (`peregrine://title/tmdb/123`, `peregrine://downloads`, …). The handler routes through the same single-instance path as an external deep link, so warm and cold starts behave identically.
- **macOS gets at most one action** on the notification body, never a set that would collapse into an "Options" menu.
- **Windows may lose the click** once the toast leaves the screen, so every notification's destination is also one click away in the in-app centre. No capability depends on the toast being clicked.
- `id` is an int32 from a persisted rotating counter, because the plugin requires a 32-bit integer.

### 7. Privacy of notification content

Toast content lands on lock screens and in notification centres that other people can see.

- Default: the media title appears, because that is the point of the notification.
- Setting: **"Hide titles in notifications"** replaces the subject with a generic string ("Download finished") for users who share a screen.
- Adult-flagged titles never appear in a notification body or title, regardless of settings.
- No notification ever contains a filesystem path, an API key, a magnet URI, a tracker URL, or an infohash.

## Consequences

Positive: one event pipeline serving both the OS and the app, so nothing is lost to a missing daemon or a muted app; per-event sound and enablement that users can actually reason about; no bundled audio; honest behaviour on the platforms whose limits we cannot fix; and a notification system that stays quiet while the user is already looking at the thing.

Negative and accepted:

- We cannot guarantee silence on Linux and Windows, and we say so instead of implying otherwise.
- Nothing can be retracted on macOS and Windows, so a wrong notification becomes a stale centre entry we can only mark superseded in-app.
- Click routing on Windows is unreliable by platform design, which is why the in-app centre is authoritative; this costs a screen that a toast-only design would not need.
- Two extra tasks of real work (the centre and the settings section) beyond "call the notification API".
- The security-gate notification is deliberately not disableable, which is a small amount of paternalism we accept because the alternative is silently blocking a torrent the user asked for.

## Alternatives rejected

| Alternative | Why rejected |
| --- | --- |
| OS notifications only, no in-app centre | Verified to lose events: no daemon on some Linux systems, app muted on macOS, `remove_active` unsupported, Windows clicks lost. The user would miss the very thing they asked to be told about |
| In-app toasts only | The requirement is native notifications; an app that only speaks while it is on screen is worse than useless for a 40-minute download |
| One global on/off switch | The event set is heterogeneous: a finished download is wanted, a finished metadata sync is noise. Per-event control is the difference between a useful feature and one users disable entirely |
| Ship our own notification sound files | Installer weight and licensing questions for something the OS already provides; the JAR/OGG licensing minefield in particular is not worth entering for a chime |
| Poll or use `group` for coalescing | Grouping is documented as ignored on desktop; the plugin simply drops it |
| Bypass Do Not Disturb for "important" events | Hostile behaviour, and it breaks the OS contract users rely on. Critical events still respect DND and surface in the centre |
| Request permission at first launch before any user action | Prompting for something the user has no context for trains them to deny it. We ask in the wizard and at the first relevant action |

## Verification

`SPIKE-11` (1 day, on real hardware per OS) must establish, and the results are recorded here:

1. The exact working sound value for each OS and each event class, including whether an omitted sound is actually silent on macOS, Windows, and two common Linux daemons (GNOME Shell and dunst).
2. Whether macOS shows a system prompt from `Notification.requestPermission()` in a packaged, signed build, and what `isPermissionGranted()` reports before and after.
3. Linux behaviour when no notification daemon is present: does the call error, hang, or silently succeed, and does the in-app centre correctly become the only channel.
4. Windows click reporting window, so the in-app centre fallback can be described accurately in the UI copy.

Phase 0 exit criteria gain one line: a download that completes while the app is unfocused raises exactly one notification, with sound when enabled, that routes to the title when clicked.
