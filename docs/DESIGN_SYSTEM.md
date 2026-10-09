# Peregrine — Design System

The visual and interaction contract. Platform conventions and their sources are in [research/desktop-ux.md](./research/desktop-ux.md); this document is the implementation spec that the UI is built and reviewed against.

Two product constraints shape everything here: Peregrine must not look like a website in a window, and it must feel instant. Both are enforced by tokens and rules rather than by taste.

---

## 1. Brand

### 1.1 The mark

The peregrine falcon is the fastest animal alive; the product's promise is speed. The mark is a wing sweep that doubles as a play triangle, built from three geometric strokes on a 32-unit grid so it stays legible at 16 px and scales cleanly to a 1024 px app icon.

| Element | Rule |
| --- | --- |
| Primary mark | Monochrome-capable, single accent path. Files: `assets/branding/logo-mark.svg` |
| Small mark | Two strokes instead of three, for use below 20 px where the third stroke blurs. File: `assets/branding/logo-mark-small.svg` |
| Wordmark | Mark plus "Peregrine" set in the UI font, converted to outlines before any release |
| App icon | Mark on a platform-shaped container: macOS squircle with the standard safe margin, Windows multi-size `.ico`, Linux PNG set at 16/24/32/48/64/128/256/512 |
| Minimum size | 16 px for the small mark; below that, use the app icon container |
| Clear space | Half the mark's height on all sides |
| Colour | `--accent` on neutral surface, or fully monochrome where the platform requires it |

Don'ts: no gradients on the mark, no drop shadow inside the 16 px context, no rotation, no outline-plus-fill combination, no wordmark stretched or letter-spaced, no recolouring outside the theme's accent token.

### 1.2 Naming and voice

The product name is a common noun with no article ("Peregrine is scanning your library"). UI copy is sentence case, no exclamation marks, no anthropomorphism about failures ("The download stalled. No peers answered." is correct; "Uh oh!" is not). Errors state what happened, why, and the next action.

---

## 2. Colour

### 2.1 Three-layer tokens

```
primitive   --gray-0 … --gray-12, --blue-0 … , raw values, never used directly in components
semantic    --surface-*, --text-*, --border-*, --accent*, --state-*   ← components use only these
component   --card-bg, --overlay-scrim, --progress-track                ← only when a semantic token cannot express it
```

Every colour is authored in **OKLCH** so lightness is perceptually uniform and a theme can be derived by shifting hue and chroma without re-doing contrast work. One `--hue-brand` custom property drives the entire accent family, which is what makes user-authored themes tractable.

```css
:root {
  --hue-brand: 250;

  /* dark (default) */
  --surface-0: oklch(0.19 0.008 var(--hue-brand));
  --surface-1: oklch(0.23 0.010 var(--hue-brand));
  --surface-2: oklch(0.27 0.012 var(--hue-brand));
  --surface-scrim: oklch(0.12 0.010 var(--hue-brand) / 0.72);

  --text-primary:   oklch(0.96 0.005 var(--hue-brand));
  --text-secondary: oklch(0.78 0.010 var(--hue-brand));
  --text-tertiary:  oklch(0.65 0.010 var(--hue-brand));
  --text-on-accent: oklch(0.30 0 0);

  --border-subtle:  oklch(0.32 0.010 var(--hue-brand));
  --border-strong:  oklch(0.55 0.012 var(--hue-brand));

  --accent:         oklch(0.68 0.16 var(--hue-brand));
  --accent-hover:   oklch(0.73 0.16 var(--hue-brand));
  --accent-muted:   oklch(0.42 0.08 var(--hue-brand));

  --state-ok:       oklch(0.72 0.15 150);
  --state-warn:     oklch(0.80 0.15 85);
  --state-danger:   oklch(0.68 0.19 25);
  --state-info:     oklch(0.72 0.12 240);

  --focus-ring:     var(--accent);
}

[data-theme="light"] {
  --surface-0: oklch(0.99 0.002 var(--hue-brand));
  --surface-1: oklch(0.97 0.004 var(--hue-brand));
  --surface-2: oklch(0.94 0.006 var(--hue-brand));
  --surface-scrim: oklch(0.30 0.010 var(--hue-brand) / 0.45);

  --text-primary:   oklch(0.20 0.010 var(--hue-brand));
  --text-secondary: oklch(0.45 0.010 var(--hue-brand));
  --text-tertiary:  oklch(0.52 0.010 var(--hue-brand));
  --text-on-accent: oklch(0.99 0 0);

  --border-subtle:  oklch(0.90 0.006 var(--hue-brand));
  --border-strong:  oklch(0.615 0.010 var(--hue-brand));

  --accent:         oklch(0.55 0.18 var(--hue-brand));
  --accent-hover:   oklch(0.50 0.18 var(--hue-brand));
  --accent-muted:   oklch(0.92 0.04 var(--hue-brand));

  --state-ok:       oklch(0.55 0.14 150);
  --state-warn:     oklch(0.62 0.14 75);
  --state-danger:   oklch(0.55 0.19 25);
  --state-info:     oklch(0.55 0.14 240);
}
```

The light block lists every semantic colour whose light value differs from the dark base, because those are exactly the drift points between this example, `packages/theme/src/tokens.css`, and `assets/themes/default-light.json`.

`--text-on-accent` is chosen against the accent it sits on, not against the surface. A dark-theme accent is bright (`oklch(0.68 …)` in Default Dark, `oklch(0.66 …)` in Midnight), so only a dark label clears 4.5:1 and those themes carry `--text-on-accent` near `oklch(0.30 0 0)`; the light theme's darker accent takes a light label. The polarity is therefore per theme and must be re-derived whenever an accent changes.

### 2.2 Contrast requirements

Thresholds:

| Pair | Minimum |
| --- | --- |
| Body text on any surface | 4.5:1 (WCAG 2.2 AA) |
| Large text and headings ≥ 24 px | 3:1 |
| Icons and control borders that carry meaning | 3:1 against the adjacent surface |
| Progress and state indicators | 3:1, and never colour alone: pair with icon or text |
| Focus ring | 3:1 against both the component and the background |

`xtask check-theme` computes contrast for every semantic pair in every shipped theme and fails the build on a violation. The shipped values failed the matrix below before this change, so the matrix is the enforced reading of the thresholds above rather than a guideline:

| Foreground | Background | Minimum |
| --- | --- | --- |
| `--text-primary`, `--text-secondary`, `--text-tertiary` | `--surface-0`, `--surface-1`, `--surface-2` | 4.5:1 |
| `--text-on-accent` | `--accent`, `--accent-hover` | 4.5:1 |
| `--border-strong` | `--surface-0`, `--surface-1`, `--surface-2` | 3:1 |
| `--accent`, `--accent-hover` | `--surface-0`, `--surface-1`, `--surface-2` | 3:1 |
| `--state-ok`, `--state-warn`, `--state-danger`, `--state-info` | `--surface-0`, `--surface-1`, `--surface-2` | 3:1 |
| `--border-subtle` | — | excluded: decorative, no threshold |
| `--surface-scrim` | — | excluded: composited and decorative, no threshold |
| `--accent-muted` | — | excluded: decorative tint, no threshold |

Rules that make the matrix testable: every pair is measured in both orders and the worse ratio is the one that must clear its minimum; a token carrying an alpha channel is composited over its background before measuring; OKLCH is converted to linear sRGB and the WCAG 2.2 relative-luminance formula is applied. An OKLCH value outside sRGB is gamut-mapped by reducing chroma at constant lightness and hue and clipping the residual, and every pair must clear its minimum under that rendering as well as under plain per-channel clipping, so the gate and the themes cannot disagree about an out-of-gamut accent. The focus-ring row above is satisfied by requiring `--accent` against every surface, because §2.1 defines `--focus-ring` as `var(--accent)`.

APCA values are reported alongside for information, but WCAG AA is the gate because that is what the platform accessibility tooling reports to users.

### 2.3 System accent integration

With "follow system accent" on, the app reads the OS accent (macOS `NSColor.controlAccentColor`, Windows accent palette, GNOME accent, KDE colour scheme) and maps it to `--accent` while deriving `--hue-brand` from it so the neutral surfaces stay harmonious. Contrast is re-validated after mapping; if the system accent fails, the app keeps it for decoration but not for text-bearing fills.

---

## 3. Typography

| OS | Stack |
| --- | --- |
| macOS | `system-ui, -apple-system, BlinkMacSystemFont, "SF Pro Text", sans-serif` |
| Windows 11 | `system-ui, "Segoe UI Variable Text", "Segoe UI", sans-serif` |
| Linux | `system-ui, "Adwaita Sans", Cantarell, Inter, "Noto Sans", sans-serif` |

| Token | Size / line-height | Use |
| --- | --- | --- |
| `--text-display` | 34 / 40 | Hero title |
| `--text-title-1` | 24 / 30 | Screen title |
| `--text-title-2` | 19 / 26 | Section title |
| `--text-body` | 14 / 21 | Default; the density-sensitive value |
| `--text-body-strong` | 14 / 21, weight 600 | Emphasis, card titles |
| `--text-caption` | 12 / 17 | Metadata, badges, timestamps |
| `--text-micro` | 11 / 15 | Progress figures, dense tables only |

Rules: no web fonts are loaded (a font flash is the most common "this is a website" tell); numeric columns and all timecodes use `font-variant-numeric: tabular-nums`; text is never scaled by transform; layouts must survive 1.4× expansion for translation; headings use the platform's weight conventions rather than a bespoke scale.

---

## 4. Space, shape, depth

| Token family | Values |
| --- | --- |
| Space | 2, 4, 6, 8, 12, 16, 20, 24, 32, 40, 56, 72 (px) |
| Radius | `--radius-sm` 6, `--radius-md` 10, `--radius-lg` 14, `--radius-xl` 20, `--radius-full` |
| Border | 1 px `--border-subtle`; 2 px for focus; no decorative rules |
| Elevation | `--elev-1` menu, `--elev-2` popover, `--elev-3` dialog, `--elev-4` modal sheet. Elevation is a shadow plus a border, never a shadow alone |
| Density | `comfortable` (default) and `compact`, applied via a root attribute that changes space and type tokens, never component internals |
| Target size | ≥ 28 px for pointer targets, ≥ 44 px for touch-capable surfaces |

Cards use the same radius and shadow at every size; a poster is never individually styled. Fixed tile aspect ratios (2:3 poster, 16:9 still) are what keep virtualization and the scrollbar stable.

---

## 5. Motion

```css
:root {
  --dur-1: 80ms;    /* press, hover tint, focus ring */
  --dur-2: 120ms;   /* icon swap, checkbox, tooltip in */
  --dur-3: 180ms;   /* menus, popovers, toasts, card lift */
  --dur-4: 240ms;   /* sheets, dialogs, detail chrome in */
  --dur-5: 320ms;   /* hero expansion, rail cross-fade */

  --ease-standard:   cubic-bezier(0.2, 0, 0, 1);
  --ease-decelerate: cubic-bezier(0, 0, 0, 1);
  --ease-accelerate: cubic-bezier(0.3, 0, 1, 1);
  --ease-emphasized: cubic-bezier(0.05, 0.7, 0.1, 1);
}

@media (prefers-reduced-motion: reduce) {
  :root { --dur-1: 1ms; --dur-2: 1ms; --dur-3: 1ms; --dur-4: 1ms; --dur-5: 1ms; }
}
```

The same numbers are exported as a TypeScript object so imperative animation and CSS never diverge. Rules:

1. Use `--dur-1`/`--dur-2` for what the user is touching, `--dur-3` for things that replace each other, `--dur-4`/`--dur-5` for whole surfaces only.
2. Animate `transform` and `opacity` only. A lint rule rejects animating layout properties in feature code.
3. `will-change` is applied when an interaction starts and removed when it settles, never left on a grid of 500 tiles.
4. Motion explains a spatial relationship or it does not happen. No decorative motion, no bouncing, no spring on a hover.
5. `prefers-reduced-motion` removes non-essential motion and collapses durations to 1 ms; essential state changes become instant rather than absent.

### The hover-preview ladder

The Netflix-style preview is the most-scrutinized interaction in the app, so it is specified exactly:

| Step | Behaviour |
| --- | --- |
| Enter | Scale to 1.06 over `--dur-3` (180 ms) with `--ease-decelerate`; scale is hard-capped at 1.12 or adjacent cards collide. The shadow ramp rides a pseudo-element's opacity over `--dur-3`, never an animated `box-shadow` |
| 450 ms dwell | Cleared by pointer leave, scroll, `visibilitychange`, or the card dropping below 60% visibility (`IntersectionObserver` threshold 0.6). Only then does the preview start |
| Preview | Media fades in over `--dur-4` (240 ms) at opacity only. If the trailer is not buffered, play a pre-generated 6–10 s silent loop — never block the zoom on the network. Metadata sits inside the scaled layer so it moves with the card |
| Exit | 120 ms `--ease-accelerate` back to scale 1, then clear the video (`src = ''` followed by `load()`) so decoders do not accumulate across a long browse session |
| Keyboard parity | Focus reveals the metadata panel only. Preview is pointer-only and must never trigger on focus |
| Budget | No layout thrash: the card keeps its geometry, the preview is an absolutely positioned layer inside fixed bounds, and `will-change` is applied on enter and removed on `transitionend` |

### Shared-element transitions

A poster to detail transition uses a view transition group: the poster animates over `--dur-5 var(--ease-emphasized)`, the detail chrome fades in over `--dur-4`, and the backdrop cross-fades on a separate layer so it never unmounts mid-transition. Placeholders fade out over `--dur-3` only after the real image has decoded.

---

## 6. Component inventory

`packages/ui` owns these; a feature never re-implements one.

| Group | Components | Required behaviour |
| --- | --- | --- |
| Primitives | Button, IconButton, Input, Textarea, Select, Combobox, Checkbox, Radio, Switch, Slider, Tabs, Tooltip, Popover, Menu, Dialog, Sheet, Toast, Badge, Avatar, Progress, Skeleton | Full keyboard operation, per-platform focus ring, disabled and loading states, reduced-motion branch |
| Collection | Grid, Rail, List, Table, FilterBar, SelectionBar, EmptyState, ErrorState | Virtualized where unbounded, keyset paging, stable scroll anchor, selection with shift and cmd/ctrl |
| Domain | TitleCard, EpisodeRow, SeasonHeader, CastStrip, AvailabilityBadge, QualityBadge, ProgressRing, JobRow, DeviceRow, TrackRow, SubtitleCandidateRow | Driven only by IPC DTOs; no domain rules; every state designed (idle, hover, focus, downloading, blocked, failed, missing) |
| Player overlay | ControlBar, Timeline, ChapterMarkers, TrackMenu, SubtitleMenu, NextUpCard, BufferIndicator, TelemetryHud | Auto-hide on inactivity, reappear on any input, never captures focus from the video surface accidentally |
| Chrome | TitleBar, Sidebar, CommandPalette, SettingsSection, Toasts, ConfirmDialog | Matches platform conventions per OS; the palette is the complete action surface |

Each component ships with a static gallery entry used for visual regression, plus an accessibility note and its animation tokens.

---

## 7. Theming

### 7.1 Shipped themes

| Theme | Intent |
| --- | --- |
| Default Dark | The primary experience: `--hue-brand: 250`, neutral blue-violet surfaces |
| Default Light | Same geometry and accent, light surfaces, validated contrast |
| Midnight | Higher contrast dark with a deeper base and a dimmer accent for dark rooms |
| High Contrast | Maximum contrast for low-vision users; separate from the OS forced-colours path, which is honoured independently |
| System | Follows the OS light/dark and accent; the default on first run |

### 7.2 Theme file format

A theme is a JSON document with a schema version, metadata, and token overrides. Files live in `assets/themes/` and user themes in the app config directory.

```jsonc
{
  "schemaVersion": 1,
  "id": "midnight",
  "name": "Midnight",
  "author": "Peregrine",
  "appearance": "dark",
  "hueBrand": 262,
  "tokens": {
    "surface-0": "oklch(0.14 0.010 var(--hue-brand))",
    "surface-1": "oklch(0.18 0.012 var(--hue-brand))",
    "accent": "oklch(0.66 0.15 var(--hue-brand))"
  },
  "wallpaper": { "kind": "gradient", "from": "oklch(0.22 0.03 var(--hue-brand))" }
}
```

Rules: unknown keys are rejected, not ignored; every theme must pass the contrast gate and the visual snapshot suite; imports are validated before being applied; a theme can never change geometry, motion, or behaviour, only colour and wallpaper. The authoring guide lives in the same folder as the shipped themes so a community theme is a pull request with one file.

---

## 8. Platform adaptation

Per-OS chrome, menu, scrollbar, dialog, and context-menu specifics are specified in [research/desktop-ux.md](./research/desktop-ux.md) §1 and summarised here only as implementation obligations:

| Obligation | Where it lands |
| --- | --- |
| Platform window chrome and drag regions | `TitleBar` with a per-OS variant; never a fake web titlebar with invented buttons |
| macOS app menu, Edit menu roles, Settings at `Cmd+,` | Shell menu registry, verified by the OS-conventions test |
| Reveal-in-file-manager wording per OS | `RevealInFileManager` action with platform strings |
| Overlay scrollbars that auto-hide; no custom scrollbar skin | Global style; a screenshot test asserts no permanent scrollbar track |
| Native-feeling scroll physics and no rubber-band mismatch | Do not intercept wheel or scroll events; use the native scroller |
| Focus rings that match the platform | `:focus-visible` styles per OS, not a universally applied outline |
| Native dialogs for file and folder selection | Tauri dialog plugin only; no HTML file input |
| Forced-colours and high-contrast modes honoured | `forced-colors` media query branch in tokens |

---

## 9. Iconography

One outlined family at a consistent 1.5 px stroke on a 24 px grid, with filled variants reserved for active states. Sizes are 16, 20, 24, 32, and no others. Every icon has a text label or an accessible name; emoji are never used as icons (lint gate). Product-specific marks (the falcon, availability states, quality badges) are authored in the same grid so they sit correctly next to generic icons.

---

## 10. Accessibility requirements

| Requirement | Standard |
| --- | --- |
| Keyboard operability | Every action reachable without a pointer; roving tabindex in grids and rails; LIFO Escape layering; focus restored on close |
| Screen readers | Grid announces position and count ("Row 3 of 12, item 7 of 20"); player announces state changes; images have meaningful alt text or are marked decorative |
| Contrast | WCAG 2.2 AA in every shipped theme, machine-verified |
| Motion | `prefers-reduced-motion` collapses durations and removes non-essential motion |
| Transparency | `prefers-reduced-transparency` removes blur and scrim layers in favour of solid surfaces |
| Text scaling | Layouts hold at 80%–150% UI scale and 1.4× text expansion |
| Captions | Size, colour, background, and position are user-configurable and persist |
| Target size | Minimums in §4 enforced by a layout test |

---

## 11. The ten rules that keep it from feeling like a website

The full anti-pattern list with fixes is in [research/desktop-ux.md](./research/desktop-ux.md) §4. These ten are review-blocking:

1. No browser-style back button; navigation is the sidebar, menus, and shortcuts.
2. No full-page spinners. Ever. Cached content or a skeleton with real geometry.
3. No hamburger menu on desktop; a menu bar or a labelled sidebar.
4. No custom scrollbar skin; the platform's overlay scrollbar behaviour wins.
5. No hover-only affordance without a keyboard path.
6. No route transition that unmounts the shell or causes a flash.
7. No HTML `alert`, `confirm`, or `prompt`; use the app's dialog primitives.
8. No web font loading, no layout shift on font swap.
9. No `Cmd`/`Ctrl` mix-ups: every accelerator is registered per platform and shown with the platform's modifier glyph.
10. No ignored OS state: window geometry, accent colour, appearance, reduced motion, and high contrast are all honoured.

---

## 12. Asset and validation pipeline

| Asset | Source | Validation |
| --- | --- | --- |
| Logo and icon | `assets/branding/*.svg` | Rendered at 16/32/128/512 px in a snapshot test; safe-margin check per platform |
| Avatar marks | `assets/avatars/*.svg` | Rendered at 24/32/64/128 px in the same snapshot test; originality and `NOTICE` attribution checked before release |
| Platform icon bundles | Generated by `xtask icons` into `apps/desktop/src-tauri/icons` | Size set and format asserted by `xtask icons --check` |
| Themes | `assets/themes/*.json` | Schema validation, contrast gate, visual snapshot per theme |
| Component gallery | Static build of `packages/ui` | Visual regression baseline per theme and OS |
| Screenshots for docs | Scripted from the E2E run, never hand-taken | Refreshed per release |

`xtask check-theme` and `xtask icons --check` are part of the standard gate set in `AGENTS.md` §3.

---

## 13. Notifications and sound

Delivery policy, consent, and the per-event catalogue are specified in [adr/0015](./adr/0015-native-notifications.md). This section covers only how they look, sound, and read.

### 13.1 In-app notification centre

The authoritative surface, because OS delivery cannot be relied on:

| Element | Spec |
| --- | --- |
| Entry point | Bell in the top bar with an unread dot (never a number above 9; show `9+`), plus the same list in Settings → Notifications → History |
| List | Grouped by day with sticky headers, newest first, virtualized, dismissible by row and by "mark all read" |
| Row | Severity icon, title, one-line body, relative time, and up to two actions (Open, Dismiss). Critical rows carry the `--state-warn` or `--state-danger` accent on their icon only, never a full-row tint |
| Click | Routes the entry's deep link through the same path as a toast click, then marks it read |
| Empty state | One line, no illustration: "Nothing yet. Download and update events will appear here." |
| Sound | The in-app chime plays only when a new entry arrives and the master sound switch is on; never on read, dismiss, or history browsing |

### 13.2 Toast fallback

Used for in-app-only mode, or when OS notifications are unavailable:

| Property | Spec |
| --- | --- |
| Placement | macOS top-right below the title bar; Windows and Linux bottom-right above the taskbar. The platform's own notification position wins when OS notifications are in use, because then we are not drawing it |
| Width | 360 px fixed; body clamps to two lines with a middle ellipsis |
| Lifetime | 6 s for normal, 10 s for critical, never auto-dismissed while hovered or focused |
| Animation | Enter: 16 px slide plus fade over `--dur-4 var(--ease-decelerate)`. Exit: fade over `--dur-3 var(--ease-accelerate)`. With `prefers-reduced-motion`, opacity only |
| Stacking | Maximum three visible; a fourth collapses into the oldest as a "+N more" line |
| Interaction | Click opens, middle-click or swipe dismisses, focus moves to the toast only for critical events |
| Keyboard | Reachable from the centre and via a shortcut; never steals focus silently |

### 13.3 Sound design

| Concern | Decision |
| --- | --- |
| Assets | None bundled. OS-provided sounds by default, per-OS mapping in [adr/0015](./adr/0015-native-notifications.md) |
| In-app chime | Synthesized WebAudio, two sine tones, 180 ms total, peak about −18 dBFS, no reverb. Success is a rising interval; failure is the same shape a minor third lower. No asset file, no licence question, identical on every OS |
| Rules | Never plays for a suppressed event; never plays when the master switch is off; never overlaps itself (a second event within 300 ms reuses the sounding cue); respects system mute and never raises its own volume |
| Custom sounds | Optional per OS, validated by extension and size; a load failure falls back to the default and says so once, without a modal |

### 13.4 Settings → Notifications

A single section, registry-driven like every other, in this order:

1. **Master switch** — "Notify me about events", with the resolved permission state beneath it and a *Send test notification* button.
2. **Per-event rows** — name, one-line description, enable toggle, sound toggle. The catalogue and defaults are in [adr/0015](./adr/0015-native-notifications.md). The security-gate row has its toggle disabled with the reason shown inline rather than hidden.
3. **Quiet hours** — start, end, and "deliver a digest when they end". Off by default.
4. **Appearance of content** — "Hide titles in notifications" with the lock-screen reason in the description.
5. **Sound** — master sound switch, default sound preview, custom sound picker.
6. **Delivery notes** — one honest line per OS: on Linux and Windows, silence and delivery depend on the system notification settings, with a button that opens them.

Copy rules: sentence case, no exclamation marks, title ≤ 60 characters and body ≤ 120 (the OS truncates silently beyond that), and the body always states what happened rather than how the app feels about it.

---

## 14. Monitoring and agent access

Policies live in [adr/0016](./adr/0016-monitoring-and-availability.md) and [adr/0017](./adr/0017-mcp-server.md). This is how they appear.

### 14.1 Monitor state, everywhere it appears

A monitor's state is a text chip with an icon, never a colour alone:

| State | Chip | Where |
| --- | --- | --- |
| `scheduled` | Neutral outline, "Coming 12 Nov" | Detail page, watchlist row, calendar |
| `airing` | Neutral outline with a progress dot, "Airing weekly" | Detail page, calendar |
| `released` | `--state-info` tint, "Aired, no release yet" | Detail page, calendar, notifications |
| `release_found` | `--state-ok` tint, "Available" | Detail page, queue link, notifications |
| `in_library` | Solid check, "In library" | Everywhere, replaces the download action |
| `paused` | Dimmed, "Paused" | Monitor management only |
| `ended` | Dimmed, "Ended" | Detail page |
| `blackholed` | Hidden by default, revealed in monitor settings | Monitor management only |

Because the state is derived from recorded evidence, hovering the chip reveals the reason in a tooltip: the provider field, the date, or the release that caused it. A chip without an explainable reason is a bug.

### 14.2 Calendar

Month and week views, filterable by type and by monitor state, with three derived lists beside it: **aired but not in the library**, **below the quality cutoff** (upgrade candidates), and **waiting longer than expected**. Density follows the app's density setting; the week view uses fixed-height rows so scrolling never re-measures. Clicking any entry opens the title; the whole surface is keyboard-navigable with the same roving-tabindex model as the grid.

### 14.3 Settings → Agents

| Element | Spec |
| --- | --- |
| Master switch | Off by default, with one line stating what enabling it exposes. Never enabled by a migration, never by an update |
| Client list | Name, transport, scopes, paired date, last seen, and a revoke action. Revoked clients stay listed, dimmed, until removed |
| Pairing | A six-character code shown with the requesting client name, an explicit Allow button, and a scope selector that defaults to `read` |
| Scope editor | One switch per scope with a plain-language description of what it grants |
| Activity | The `mcp_call` log, filterable by client, tool, and outcome, with denials and approval requests surfaced at the top |
| Setup snippets | A copy-paste configuration block per common client, generated from the app's own path, so stdio setup is not guesswork |
| Transport | stdio always listed; Streamable HTTP behind its own toggle with the port and token shown |

Approval dialogs name the requesting client in the title ("Grok bot wants to download"), state the title, release, size, and quality, and offer Deny, Approve once, Approve for this title, and Approve for this session — with the remembered choice shown as an expiring chip so it is never invisible.

---

## 15. Sharing and devices

Policy is in [adr/0018](./adr/0018-media-server.md). This is the surface.

### 15.1 Share setup

| Element | Spec |
| --- | --- |
| Destination | Folder picker, then a plain sentence: "Peregrine will link your media here. Jellyfin and Plex read this folder; they never write to it." |
| Disk cost | A preview before anything is written: "42 GB of links, no extra disk used" for hardlink mode, or the real copy cost when the destination is on another filesystem, with the reason stated |
| Mode | Hardlink or copy, with the filesystem constraint explained inline rather than discovered as an error |
| Contents | Four switches — NFO files, artwork, subtitles, placeholders — each with a one-line consequence |
| Library scope | Which libraries this share exports; the destination is never above the folder the user chose |
| Run | A job with progress, a cancel, and a result line counting exported, updated, skipped, and failed items with a link to the log |
| Finish | The exact folder path in a copyable field, plus the one-line instruction per ecosystem: add this folder as a Movies or Shows library |

### 15.2 Devices

A table: name, kind, scopes, paired, last seen, revoke. Pairing shows a QR code, a short URL, and an optional PIN on one screen; the token is never displayed. A newly paired device defaults to browsing and streaming only the shared libraries.

### 15.3 Now playing

Per connected device: what it is streaming, the delivery mode (direct, remux, or transcode), throughput, and a disconnect action. When a stream transcodes, the reason is shown — "client cannot play H.265" is actionable, "transcoding" is not. When the host has no encoder and therefore cannot transcode, the row says so rather than failing silently.

### 15.4 Network diagnostics

One card, reachable from the sharing screen, that answers "why can't my TV see it?" without a forum post:

1. Detected interfaces and their addresses, with the primary LAN address highlighted and VPN and container bridges labelled and not advertised by default.
2. Whether the server is listening, on which addresses, and whether mDNS discovery is up.
3. A reachability probe result naming the three common causes in order: access-point isolation, a guest network, or a firewall.
4. One honest line about Wi-Fi: it works, and for 4K files a wired server with wireless clients is the setup that never buffers.

Copy rules: no DLNA or HTTP specification jargon, no "simply", and every failure states the next action.

---

## 16. Accounts, avatars, and parental controls

Policy and the honest security boundary are in [adr/0019](./adr/0019-local-accounts-and-parental-gating.md). This is the surface.

### 16.1 Who's watching

The launch screen when more than one account exists: a centred row of avatars, name beneath each, keyboard-navigable left to right, `Enter` to select. Account order is user-controlled with the default first. A locked account shows a small lock glyph, not a different interaction — selecting it raises the gate. No teaser content, no counts, no "recently watched" hints that could leak who watched what. With one account, the screen is skipped entirely.

### 16.2 Avatars

Four sources, offered in this order in the picker because that is the order of least effort:

| Source | Behaviour |
| --- | --- |
| **Initials** | The default. The account's initial set over a themed geometric shape in the account's accent colour, generated at render time — no asset, and it still looks chosen rather than blank |
| **Built-in set** | Eight original marks shipped with the app (`assets/avatars/`), drawn on the logo's geometric language and previewed at the size they will actually appear. Like the logo, each is authored at two weights — full detail above 32 px, a simplified glyph below it — because a thin arc collapses into a dot at switcher size |
| **Your own image** | A file from disk, cropped square with a drag handle, previewed live. PNG, JPEG, or WebP only; SVG and animated formats are refused with a reason, not a silent failure; EXIF and GPS are stripped on import |
| **Generic** | A neutral silhouette, used when nothing is chosen and for the guest account |

Rendered locally like any other artwork, stored with a blurhash, never fetched from anywhere. Six sizes: 24 px switcher chip, 32 px list row, 40 px comment and now-playing row, 64 px picker tile, 96 px gate sheet, 128 px account settings header.

### 16.3 The switcher

A chip in the top bar: avatar plus name. Selecting it opens a small panel listing the other accounts with their lock state. Switching to or from a restricted account raises the gate; the panel never reveals what is behind it.

The gate itself is a centred sheet with the avatar, the account name, and one line of reason ("Enter your device password to switch to Daniel"). It shows which method is being used, cancels cleanly, and after repeated failures shows a countdown rather than a scolding. When no OS method exists it says exactly that, and that the app PIN in use is weaker.

### 16.4 Parental settings

A section visible only to accounts with `can_manage`:

| Element | Spec |
| --- | --- |
| Account list | Kind, rating ceiling, curated-only flag, capabilities, and lock policy per account, with a plain-language summary: "Can watch up to PG-13 · Cannot download · Cannot see the download queue" |
| Curation | Two halves: a rating ceiling with the resolved examples shown ("PG-13 and below"), and a curated collection the parent fills by adding titles from the normal browse surface, with a badge on the grid for what a child account can see |
| Requests | A pending queue with the requesting child's avatar, the title, and Approve or Deny with an optional reason. Approving starts the normal acquisition flow |
| Lock policy | Per account: none, app PIN, or OS authentication; idle auto-lock; and the capability summary above |
| The boundary note | One sentence, once, in a quiet style: "This stops account switching in Peregrine. It does not protect the files on this computer — for that, use separate computer accounts." Never a modal, never repeated |

### 16.5 What a child account sees

Nothing is struck through, greyed out, or labelled as restricted. Blocked content is simply absent — no locked posters, no "ask your parent" overlays on titles they cannot see. The single exception is the request affordance on a title they *can* see but which has no release yet, where "Ask" replaces "Download" and one line explains that an adult will decide. A denied request states the parent's reason, because a bare refusal teaches children to route around the system.


### 16.6 Making an account

One screen, three fields, and no more:

1. **Name** — a single text field, trimmed, 32 characters, unique within the installation, with the duplicate case explained inline rather than on submit.
2. **Avatar** — the four sources above as a row of tabs, with the current choice previewed at picker-tile size.
3. **Kind** — Adult, Child, or Guest, each with one line of consequence: "Child accounts cannot download; an adult approves requests" and "Guest keeps no history and needs no password".

The first account is created inside the first-run wizard and is always an adult; later accounts come from the switcher or parental settings. A child account created without a ceiling or curation is refused at this step with an explanation, because the schema would reject it anyway and a form should not let you build something the database will not accept.

Guest is **one account, created once by an adult, and never created twice** — the database enforces that with a partial unique index, so a second attempt fails rather than quietly making another. Its name is a fixed label rendered from a translation key rather than stored text, and its avatar is the generic mark, not changeable: a shared account should not accumulate a person's name or picture. Its content ceiling defaults to the most restrictive child account's, and settings note that widening it does not create a way around the gate, because a guest session cannot reach a broader scope without authenticating.
