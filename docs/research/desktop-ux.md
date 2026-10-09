# Peregrine — Desktop UX & Design-System Research

Tauri v2 (WKWebView / WebView2 / WebKitGTK) + React. Goal: feels native, 60/120fps, fast.
Each section ends with a **Recommendation**. URLs resolve in [§6](#6-sources).

Tauri facts used below: `WindowConfig` has `decorations`, `transparent`,
`titleBarStyle: "Visible"|"Transparent"|"Overlay"`, `hiddenTitle`, `trafficLightPosition`, `shadow`,
`windowEffects`, `backgroundColor`, `visible`, `dragDropEnabled`, `scrollBarStyle: "default"|"fluentOverlay"`
(Windows-only), `backgroundThrottling` (Linux/Windows **unsupported**), `zoomHotkeysEnabled`, `theme`;
platform overrides merge from `tauri.macos.conf.json` / `tauri.windows.conf.json` / `tauri.linux.conf.json`.

---

## 1. OS conventions

### 1.1 Window chrome

| Concern | macOS | Windows 11 | GNOME | KDE |
|---|---|---|---|---|
| Choice | `titleBarStyle:"Overlay"`, `hiddenTitle:true`, `decorations:true` | `decorations:true` + Mica; custom bar only if content needs it | `decorations:true`; app-drawn Adwaita header bar | `decorations:true`; Breeze bar |
| Buttons | traffic lights left; reserve 78×28 | caption buttons right; reserve 138×32 | close right | close right |
| `decorations:false` | avoid — loses move/align + window menu | avoid — loses Snap Layouts + system menu | avoid — loses CSD shadow | avoid |
| `transparent:true` | needs `app.macOSPrivateApi:true` → **App Store rejection**; prefer `windowEffects` | + `shadow:false` yields 1px white border | unsupported | unsupported |
| Backdrop | `windowEffects:["sidebar"]` | Mica; Mica Alt for tabbed bars (Win11 ≥ 22000) | solid `@window_bg_color` | solid Breeze |
| Drag | `data-tauri-drag-region` | same | same | same |
| Double-click bar | zoom/maximize (system-handled) | maximize | maximize | maximize |
| Bar height | 28 (52 with toolbar) | **32**; **48** with search or avatar | 47 | ~30 |
| State | `tauri-plugin-window-state` | same | same | same |

`Overlay` caveats (Tauri schema, verbatim): height differs across OS versions; a custom drag region
is mandatory; **the window cannot be dragged while unfocused**
([tauri#4316](https://github.com/tauri-apps/tauri/issues/4316)); title colour follows system theme.
`trafficLightPosition` requires `Overlay` **and** `decorations:true`.

`tauri.macos.conf.json`: `titleBarStyle:"Overlay"`, `hiddenTitle:true`, `decorations:true`,
`trafficLightPosition:{x:16,y:18}`, `minWidth:960`, `minHeight:640`, `visible:false`.
```css
.titlebar { height: 32px; display: grid; grid-template-columns: auto 1fr auto;
  user-select: none; app-region: drag; }
.titlebar button, .titlebar input, .titlebar [role="button"] { app-region: no-drag; }
```

Windows caption glyphs (if hand-drawn): height 32, full-bleed backplates, `E921 ChromeMinimize`,
`E922 ChromeMaximize`, `E923 ChromeRestore`, `E8BB ChromeClose`. The close button's hover/pressed
colour is *always* system-defined even when every other colour is overridden, and partial colour
overrides interact badly with the user's accent preference — set all or none. Icon/title/back insets
are 16px; window icon 16×16.

**Recommendation:** three platform configs. macOS `Overlay` + `hiddenTitle` + native traffic lights.
Windows native frame with Mica extended into a 32px transparent client bar (Snap Layouts intact).
Linux native decorations, no transparency, no effects. Never `decorations:false`. Restore state with
`tauri-plugin-window-state` and create the window with `visible:false` — the plugin shows the window
after restoring state, which is also the anti-white-flash mechanism.

### 1.2 Menus and accelerators

macOS requires a real `NSMenu`; Tauri v2 exposes `@tauri-apps/api/menu`. Required structure:
**App** About · Check for Updates · Settings… (`Cmd+,`) · Services ▸ · Hide (`Cmd+H`) · Hide Others
(`Cmd+Alt+H`) · Show All · Quit (`Cmd+Q`); **Edit** Undo/Redo, Cut/Copy/Paste, Paste and Match Style,
Delete, Select All; **View** Full Screen (`Ctrl+Cmd+F`); **Playback**; **Window** Minimize (`Cmd+M`),
Zoom, Bring All to Front; **Help** with a search field.

Windows/Linux: menu bar or a `⋯` toolbar button, never a hamburger. GNOME's standard is `F10` for the
primary menu and menus sit at the *end* of the header bar.

| Action | macOS | Win/Linux |
|---|---|---|
| Settings | `Cmd+,` | `Ctrl+,` |
| Search | `Cmd+F` | `Ctrl+F` |
| Command palette | `Cmd+K` | `Ctrl+K` |
| Play/pause | `Space` (suppressed in text inputs) | same |
| Seek | `←/→` ±10s, `Shift+←/→` ±60s | same |
| Full screen | `Ctrl+Cmd+F` | `F11` |
| Close / Quit | `Cmd+W` / `Cmd+Q` | `Ctrl+W` / `Ctrl+Q` |
| Shortcuts sheet | `Cmd+/` | `Ctrl+?` |
| Sidebar / Help | `Cmd+Ctrl+S` / `Cmd+?` | `F9` / `F1` |

**Recommendation:** one `commands.ts` registry emitting both the native menu and the accelerator map
per platform. Native macOS menu bar always; Windows/Linux menu bar + `F10`. `Cmd/Ctrl+,` for settings
on all four platforms.

### 1.3 Settings UX

**Recommendation:** macOS = separate `WebviewWindow` (`settings`, 720×520, non-resizable,
`titleBarStyle:"Visible"`), toolbar-style tabs (General · Playback · Appearance · Notifications ·
Advanced · About), no OK/Cancel — apply live. Windows/Linux = in-window route with a left section
rail. All platforms write optimistically to `tauri-plugin-store` and revert on failure.

### 1.4 Context menus

Cover every card, row, rail, and field. Use `PredefinedMenuItem` for Cut/Copy/Paste/Select All so
macOS wires the responder chain. One `revealLabel` token:

| Action | macOS | Windows | Linux |
|---|---|---|---|
| Reveal | Reveal in Finder | Show in Explorer | Open Containing Folder |
| Delete | Move to Trash | Move to Recycle Bin | Move to Trash |
| Preview | Quick Look | Preview | — |

**Recommendation:** card context menu = Play · Add to Watchlist · Download · Reveal (if local) ·
Copy Link · Properties. Destructive items last, after a separator, never default. Mirror every
context action in the `⋯` overflow so keyboard users have a path.

### 1.5 System integration

| Feature | macOS | Windows 11 | Linux | Tauri mechanism |
|---|---|---|---|---|
| File dialogs | NSOpenPanel | IFileOpenDialog | GTK portal | `plugin-dialog` `open/save/message/ask/confirm` |
| Notifications | Notification Center | Toast (installed apps only) | libnotify | `plugin-notification` |
| Tray | menu bar extra | system tray | AppIndicator | `TrayIconConfig` (opt-in setting) |
| Dock/taskbar progress | dock badge / window progress | `ITaskbarList3::SetProgressState` | Unity launcher API | not exposed — in-window + notification instead |
| Drag-drop onto window | yes | yes | yes | `dragDropEnabled:true` emits `DragDropEvent`; **must be `false` for HTML5 DnD on Windows** |
| Drag onto dock/taskbar | file associations | same | same | `bundle.fileAssociations` + `HandlerRank` |
| Recent files | NSDocumentController | jump list | GtkRecentManager | own list + "Open Recent ▸" |
| Deep links | CFBundleURLTypes | registry | .desktop | `plugin-deep-link`; desktop links **must be declared in config, no runtime registration** |
| Single instance | required | required | required | `plugin-single-instance` registered **first**, focusing the main window |
| Quit vs close | `Cmd+W` hides, `Cmd+Q` quits, dock click reopens | close exits unless tray | close exits | `CloseRequested` → `prevent_close()` + `hide()` on macOS |
| Zoom hotkeys | `Cmd +/-/0` | `Ctrl +/-/0` | `Ctrl +/-/0` | `zoomHotkeysEnabled` (WebView2-native on Windows, JS polyfill 20% steps elsewhere) |

**Recommendation:** ship dialog, notification, deep-link, single-instance, window-state, os, process,
store, opener, updater. Tray and global-shortcut opt-in. No dock badge or taskbar progress until a
supported API exists.

### 1.6 Scrolling and scrollbars

- macOS/Windows 11/Linux overlay scrollbars auto-hide and appear on scroll, not hover.
- `scrollBarStyle:"fluentOverlay"` is Windows-only and needs WebView2 ≥ 125.0.2535.41; macOS/Linux
  accept only `"default"`. A webview scrollbar is always engine-drawn.
- macOS WKWebView rubber-bands. `overscroll-behavior: contain` keeps the bounce but stops chaining
  and disables pull-to-refresh/swipe-back. Linux mice have no momentum at all, by design.

`.scroller{overscroll-behavior:contain}` ·
`.rail{overflow-x:auto;overscroll-behavior-x:contain;scroll-snap-type:x proximity}` ·
`@media (prefers-reduced-motion:no-preference){html{scroll-behavior:smooth}}`.

**Recommendation:** overlay scrollbars; `fluentOverlay` on Windows; `overscroll-behavior:contain` on
every nested scroller; `scroll-behavior:smooth` only under `no-preference`. Style with
`scrollbar-width`/`scrollbar-color` only — **never `::-webkit-scrollbar`**, the single most common
"this is a website" tell. Every custom rail keeps `↑↓←→`, `PageUp/PageDown`, `Home/End`.

### 1.7 Typography

| Platform | Stack |
|---|---|
| macOS | `system-ui, -apple-system, BlinkMacSystemFont, "SF Pro Text", sans-serif` |
| Windows 11 | `system-ui, "Segoe UI Variable Text", "Segoe UI", sans-serif` |
| GNOME/KDE | `system-ui, "Adwaita Sans", Cantarell, Inter, "Noto Sans", sans-serif` |

Windows 11 ramp (px/line-height): Caption 12/16 · Body 14/20 · Body Strong 14/20 semibold ·
Subtitle 18/24 · Title 20/28 · Title Large 28/36 · Display 40/52 · Large Display 68/92. Microsoft
excludes Bold and Italic — use **Semibold**. GNOME standard classes: `body`, `heading`, `caption`,
`caption-heading`, `title-1…4`, `large-title` — name our tokens the same so a GTK-adjacent theme
drops in. GNOME HIG: use system fonts, don't hard-code sizes.

Time codes, durations, progress readouts need tabular figures or they jitter:
`font-variant-numeric: tabular-nums`. Captions: `font-size: max(16px, 3.2vh)`, weight 500, white on a
≥70% black scrim, ≤2 lines, ≤42 chars/line, bottom-anchored in the video safe area.

**Recommendation:** `system-ui` first everywhere; no webfont for chrome UI. Minimum interactive text
11px, body 14px. `tabular-nums` on every numeric readout. `minWidth: 960` so the ramp never compresses.

### 1.8 Accessibility

| Expectation | Implementation |
|---|---|
| VoiceOver / Narrator / Orca | real semantics: `<button>`, `role="grid"`, `aria-selected`, `aria-live="polite"` for toasts and progress |
| Full keyboard access | no hover-only affordance exists; every card focusable and activatable |
| Focus ring | `:focus-visible` with a 2px outline at ≥3:1; never remove without an equivalent replacement |
| Reduced motion | `@media (prefers-reduced-motion: reduce)` → keep opacity cross-fades, drop transforms/parallax |
| Reduced transparency | `window.matchMedia('(prefers-reduced-transparency: reduce)')` guarded (uneven support); solid surfaces by default, vibrancy layered on |
| High contrast | `@media (forced-colors: active)` + `forced-color-adjust`; system keywords `Canvas`, `CanvasText`, `Highlight`; Windows exposes `AccentColor`/`AccentColorText` |
| Min target | WCAG 2.5.8 AA = 24×24 CSS px; use ≥28px for desktop icon buttons |

`:focus{outline:none} :focus-visible{outline:2px solid var(--focus-ring);outline-offset:2px}` plus
`@media (forced-colors:active){:focus-visible{outline-color:Highlight}}`.

**Recommendation:** full keyboard operability is the definition of "native" — every platform's a11y
stack assumes it. Roving tabindex (§2.7), `:focus-visible` only, forced-colors styling, a
reduced-motion branch (opacity-only), a reduced-transparency branch (solid), and a `Cmd+/` sheet so
discoverability never depends on hover.

---

## 2. Motion & interaction system

### 2.1 Tokens

Material 3's raw tokens, from the generated source: `DurationShort1..4` 50/100/150/200,
`Medium1..4` 250/300/350/400, `Long1..4` 450–600, `ExtraLong1..4` 700–1000; `EasingStandard`
`cubic-bezier(.2,0,0,1)`, `EasingStandardDecelerate` `cubic-bezier(0,0,0,1)`,
`EasingStandardAccelerate` `cubic-bezier(.3,0,1,1)`, `EasingEmphasizedDecelerate`
`cubic-bezier(.05,.7,.1,1)`, `EasingEmphasizedAccelerate` `cubic-bezier(.3,0,.8,.15)`.
Those are tuned for touch. Apple's fluid-interface guidance is the corrective — immediate response,
1:1 pointer tracking, interruptibility, inherited velocity, spatial consistency — so compress the
mid-range and shorten the tail.

```css
:root {
  --dur-1: 80ms;   /* press, hover tint, focus ring */
  --dur-2: 120ms;  /* icon swap, checkbox, tooltip in */
  --dur-3: 180ms;  /* menus, popovers, toasts, card lift */
  --dur-4: 240ms;  /* sheets, modals, detail chrome in */
  --dur-5: 320ms;  /* hero expand, rail cross-fade */
  --ease-standard:   cubic-bezier(0.2, 0, 0, 1);
  --ease-decelerate: cubic-bezier(0, 0, 0, 1);        /* entering */
  --ease-accelerate: cubic-bezier(0.3, 0, 1, 1);      /* exiting */
  --ease-emphasized: cubic-bezier(0.05, 0.7, 0.1, 1); /* hero/parallax */
  --ease-spring: linear(0, 0.2653 10.3%, 0.6865 26.9%, 0.937 45%, 0.9959 72%, 1);
}
@media (prefers-reduced-motion: reduce) { :root { --dur-1: 1ms; --dur-2: 1ms; --dur-3: 1ms; --dur-4: 1ms; --dur-5: 1ms; } }
```

**Recommendation:** exactly these five durations and five easings, exported as CSS custom properties
*and* a TS `motion` object so Framer Motion and WAAPI share numbers. `--dur-1/2` for what the user is
touching, `--dur-3` for things that replace each other, `--dur-4/5` for full surfaces only. Lint
against literal duration strings in components.

### 2.2 Card hover preview

`pointerenter` starts a **450ms** dwell, cleared by `pointerleave`, scroll, `visibilitychange`, or
dropping below 60% visibility (`IntersectionObserver`, `threshold:[0.6]`). At dwell: **180ms**
`--ease-decelerate` to `scale(1.06)` (hard cap 1.12, else cards collide). Preview media fades in over
**240ms** opacity; if the trailer is unbuffered play a pre-generated 6–10s silent loop — never block
the zoom on network. Metadata lives inside the scaled layer so it moves with the card. On leave:
**120ms** `--ease-accelerate` to `scale(1)`, then `video.src=''; video.load();` so decoders do not
accumulate.

Compositor-only rule: animate **only** `transform` and `opacity`. `width/height/top/left/margin`,
`box-shadow`, `filter: blur()`, and `backdrop-filter` trigger layout/paint or force a full re-raster.
Get a shadow ramp by animating a pseudo-element's opacity:

```css
.card { position: relative; contain: layout paint style; }
.card::after { content: ""; position: absolute; inset: 0; border-radius: inherit;
  box-shadow: 0 12px 32px rgb(0 0 0 / .45); opacity: 0;
  transition: opacity var(--dur-3) var(--ease-standard); pointer-events: none; }
.card[data-hover] { transform: scale(1.06); transition: transform var(--dur-3) var(--ease-decelerate); }
.card[data-hover]::after { opacity: 1; }
```

`will-change` promotes a GPU layer whose memory tracks painted size — hundreds of MB across a
200-card grid. Apply it only to the currently animating element and remove it on `transitionend`;
`contain: layout paint style` or `content-visibility: auto` is the safe structural hint. Never read
`getBoundingClientRect`/`scrollTop`/`getComputedStyle` in a handler that also writes styles: batch
reads in one rAF and writes in the next, or let `ResizeObserver`/`IntersectionObserver` drive it.
Remaining scroll work gets `{ passive: true }` plus rAF coalescing.

**Recommendation:** dwell 450ms, scale 1.06, ramp 180ms decelerate, preview 240ms opacity, teardown
120ms accelerate. Transform/opacity only; shadows ride a pseudo-element's opacity; per-element
`will-change` removed on `transitionend`. Keyboard focus never triggers preview — it is pointer-only.

### 2.3 Shared-element transitions

```js
const rect = card.getBoundingClientRect();            // read once, before any write
if (typeof document.startViewTransition === 'function') {
  const t = document.startViewTransition(() => flushSync(() => setDetail(id)));
  await t.ready;
} else { setDetail(id); }                             // FLIP keyframes from rect
```
`::view-transition-group(poster-42){animation:var(--dur-5) var(--ease-emphasized)}` and
`::view-transition-old/new(root){animation-duration:var(--dur-4)}`.

One `view-transition-name` per transitioning card (`poster-${id}`), never on an item that can unmount
mid-transition. Old and new images must share an aspect ratio or the morph distorts. The backdrop is a
**separate non-shared layer** cross-fading over `--dur-4` while the hero expands over `--dur-5`, so
the expansion leads by ~80ms. While the full-res hero decodes, keep the shared element mounted with
its BlurHash background plus a 1.2s shimmer; on `img.decode()` resolve, swap `background-image` and
fade the placeholder over `--dur-3`. Never unmount before `transition.finished`.

**Recommendation:** `startViewTransition` as progressive enhancement, with a rAF FLIP from the
recorded rect as the base path. One transition at a time; ignore navigation requests while
`transition.finished` is pending; abort on `Escape`.

### 2.4 Parallax and scroll-driven rails

```css
@supports (animation-timeline: view()) {
  .hero-media { animation: hero-parallax linear both; animation-timeline: view();
                animation-range: entry 0% exit 100%; } }
@keyframes hero-parallax { from { transform: translate3d(0,0,0) scale(1.08); }
                           to   { transform: translate3d(0,-12%,0) scale(1.12); } }
@media (prefers-reduced-motion: reduce) { .hero-media { animation: none; } }
```

The spec defines anonymous timelines via `animation-timeline: scroll()` (nearest ancestor scroller),
`scroll(root)`, and `view()`. Fallback where unsupported (WebKitGTK, older Safari): an
`IntersectionObserver`-driven `--progress` custom property, updated at most once per rAF. Promote the
hero for the duration with `will-change: transform`, removed on `scrollend`.

**Recommendation:** `animation-timeline: view()`/`scroll()` primary, one IntersectionObserver
fallback, reduced-motion kill switch. Never a `scroll` listener that writes styles per event.

### 2.5 Images and virtualization

Ladder: BlurHash/ThumbHash → 32px LQIP (`filter: blur(12px)`, `scale(1.05)`) → poster
(`decoding="async"`, `loading="lazy"`) → hero (`fetchpriority="high"`, `decoding="sync"`).

`.rail-section { content-visibility: auto; contain-intrinsic-size: auto 420px; }` — every `<img>` also
gets `width`/`height` (or `aspect-ratio`), `loading="lazy"`, `decoding="async"`,
`fetchpriority="low"`, `background: var(--blurhash)`.

`width`+`height` or `aspect-ratio` on a fixed track is what holds CLS at zero — not the fade. Do not
lazy-load the first rail or the hero. Virtualize rails with `@tanstack/react-virtual`
(`horizontal:true`, `overscan:2`, `getItemKey`) because node count is what hurts; use
`content-visibility:auto` for whole off-screen page sections. Prefetch: posters at 2× on
`pointerenter`+120ms; trailer **manifest and first segment only** after a 250ms hover dwell, never the
whole file; trailers in full only on explicit play-intent. Non-urgent work
(`createImageBitmap` pre-decode, next-rail prefetch) via `requestIdleCallback(cb,{timeout:2000})`;
split >50ms work into chunks yielding with `scheduler.yield?.()`.

**Recommendation:** virtualized rails, section-level `content-visibility`, reserved boxes for every
image, hover prefetch for posters, explicit-intent for trailers, Workers for all heavy compute.

### 2.6 Progress, optimistic UI, destructive actions

**Ring** for a short bounded operation the user is watching (one card's download); **bar** for long,
queued, or multi-item work, because it communicates queue position. A page-level spinner is never
acceptable (§4). **Coalesce:** accumulate progress in a ref, flush to React state at most once per
frame and to the visible number at ~4Hz; round the label to whole percent, drive the bar with
continuous `transform: scaleX()`, and snap to 100% only on the completion event. Prefer a masked
`transform: rotate()` ring over `stroke-dashoffset` on large SVGs. **Watchlist toggle:** flip local
state in the click tick, fire the mutation, revert + toast on failure; never disable the button while
in flight — that is the difference between snappy and sluggish; icon swap at `--dur-2` with a small
overshoot. **Destructive:** reversible → act immediately, `role="status"` toast with **Undo** for 5s,
commit after it expires; confirmation modals only for irreversible actions (delete a downloaded file,
clear history, sign out, delete account) with the object named, a destructive button, and focus on
Cancel.

**Recommendation:** ring for single in-view downloads and bars for queues; per-frame state flush,
4Hz labels, tabular numerals; optimistic watchlist with toast+undo; modal confirmation only for
irreversible operations.

### 2.7 Focus and keyboard model

This is the highest-leverage item for a 10-foot feel on desktop.

One tab stop per rail: `tabindex="0"` on the container, `-1` on every card; the roving tabindex moves
`tabindex="0"` to the active card and `Tab` leaves the rail, while the overflow chevron is a separate
stop. `←/→` step one card; `↑/↓` move to the geometrically nearest card in that direction computed
from grid coordinates, not DOM order, so ragged last rows behave; `Home`/`End` jump to the ends;
`PageUp/PageDown` move a viewport and call `scrollIntoView({block:'nearest',inline:'nearest'})`.
`Enter` activates, and `Space` is play/pause only when the player or nothing focusable holds focus so
a focused card never double-triggers. Escape is one LIFO stack via `useEscapeStack()`: context menu →
popover → search → fullscreen → detail → clear selection, never competing `keydown` listeners. Every
overlay records `document.activeElement` on open and restores it on close with `{preventScroll:true}`;
detail back-navigation restores focus to the originating card by stable `id` and centres it. Show
`:focus-visible` when moving by keyboard, never on mouse click.

**Recommendation:** roving tabindex per rail, geometric arrow navigation, `Home/End/PageUp/PageDown`,
a single LIFO Escape stack, recorded focus restore on every overlay and route change. A mouse-only
interaction is an unfinished feature.

---

## 3. Perceived performance in a webview

- **First paint from cache.** Persist the last good feed snapshot (rails, posters, scroll offsets,
  progress) in `plugin-store` or IndexedDB and render it on frame one, before any network. Create the
  window with `visible:false`, then `getCurrentWindow().show()` from a rAF after mount. Set
  `windows[].backgroundColor` to `--surface-0` so pre-CSS frames match.
- **Skeleton → content without shift.** Skeletons must occupy final geometry exactly (same
  `aspect-ratio`, track height, padding), then swap in place.
- **No white flash.** macOS `Overlay` + `windowEffects` + `backgroundColor`; Windows Mica +
  `backgroundColor` (and `noRedirectionBitmap` if transparent); Linux `backgroundColor` +
  `visible:false` until ready.
- **Resize.** Drive layout from a CSS var set by `ResizeObserver` (`--rail-width`), never from JS
  measuring children. Use `grid-template-columns: repeat(auto-fill, minmax(160px,1fr))` and let the
  engine do the work; avoid `scale()` on resize. `minWidth:960, minHeight:640`.
- **Never block.** Ranking, transcript parsing, subtitle conversion, and pre-decode go in Web Workers
  over `postMessage`, keeping tasks <50ms. Do **not** set `backgroundThrottling:"disabled"` by
  default — unsupported on Windows/Linux and it burns battery. Use
  `navigator.locks.request()` to keep playback and downloads alive instead.

Budget: first contentful paint <200ms after window show; input-to-next-paint <50ms during scroll;
zero feed layout shift.

### 3.1 Where the webview genuinely differs

| Area | Difference | Mitigation |
|---|---|---|
| Scroll physics | Chromium/WKWebView momentum ≈ native; WebKitGTK's is not, and Linux mice have none | no wheel listeners, no JS smoothing, shallow scroll containers, `overscroll-behavior:contain` |
| Text rendering | no subpixel AA on modern macOS; ClearType on Windows; freetype hinting on Linux | no negative letter-spacing below 12px, whole-pixel sizes, test at 100/125/150% |
| Refresh rate | compositor-vsync driven, so 120Hz panels get 120fps rAF; Linux X11 1× is often 60Hz | time with `performance.now()` deltas, never frame counts |
| GPU compositing (Linux) | WebKitGTK's DMABUF renderer fails on some NVIDIA/Wayland setups: blank window, resize flicker, resize crash, `AcceleratedSurfaceDMABuf was unable to construct a complete framebuffer`, `Gdk-Message: Error 71 … dispatching to Wayland display` | in order: `nvidia_drm.modeset=1`; `__NV_DISABLE_EXPLICIT_SYNC=1`; `WEBKIT_DISABLE_DMABUF_RENDERER=1`; last resort `WEBKIT_DISABLE_COMPOSITING_MODE=1`. Apply only when the symptom is confirmed |
| WebGL/canvas | WebGL2 context creation succeeds on software rasterizers and `WEBGL_debug_renderer_info` is masked, so it is undetectable from JS | no WebGL in core UI; DOM/CSS only; the player feature-detects capability, not renderer identity |
| Scrollbars / zoom | macOS+Linux ignore `scrollBarStyle`; zoom is WebView2-native on Windows but a 20%-step JS polyfill elsewhere | overlay scrollbars; integer zoom factors; test 80/100/125/150% |

**Recommendation:** ship a first-paint-from-cache pipeline, `visible:false` until painted, matching
`backgroundColor`, Workers for heavy work, and a Linux probe that applies DMA-BUF workarounds only on
confirmed symptoms — an unconditional override disables a fast path for everyone.

---

## 4. Anti-patterns

| # | Anti-pattern | Fix |
|---|---|---|
| 1 | browser-style back/forward buttons in chrome | native menus; `Cmd+[`/`Cmd+]`; one in-content back inside detail only |
| 2 | full-page loading spinner on route change | skeletons matching final geometry; keep chrome mounted |
| 3 | hamburger menu on desktop | native menu bar (macOS required), `F10` primary menu on GNOME, `⋯` overflow |
| 4 | custom scrollbars ignoring the OS | leave overlay scrollbars alone; `scrollbar-width`/`scrollbar-color` only |
| 5 | hover-only affordances with no keyboard path | every hover control also `Tab`/arrow reachable and in the context menu |
| 6 | route transitions that flash white | shell-level cross-fade; window `backgroundColor`; no white routes |
| 7 | sticky web header with shadow-on-scroll | real toolbar with a top border appearing only once content scrolls beneath |
| 8 | 300ms tap delay / double-tap-to-zoom | `touch-action: manipulation`; press feedback on `pointerdown` |
| 9 | cookie-banner-style modals | native `plugin-dialog` for blocking decisions; inline banner otherwise |
| 10 | non-native `<select>` / `<input type=file>` | `plugin-dialog` `open`/`save`; custom listbox for choices |
| 11 | ignoring OS accent and light/dark mode | `prefers-color-scheme`, `AccentColor`, `-apple-system-control-accent`, `@accent_bg_color` |
| 12 | ignoring window state | `plugin-window-state` + `visible:false` to avoid the flash |
| 13 | right-click does nothing | context menu on every card, row, rail, field |
| 14 | no macOS menu bar | full `NSMenu` per §1.2, including Edit and Window |
| 15 | text selection everywhere | `user-select:none` on chrome; `text` only in content and inputs |
| 16 | webfont flicker on first paint | system fonts for chrome; `font-display: optional` + preload if unavoidable |
| 17 | zoom that breaks layout | integer zoom steps, container queries, no fixed-px assumptions at 80% |
| 18 | momentum-less or JS-smoothed scrolling | native overflow scrolling; never intercept `wheel` |
| 19 | emoji as icons | SF Symbols / Segoe Fluent Icons / GNOME symbolic / Breeze SVGs |
| 20 | oversized mobile touch targets | 28–32px controls; hit area ≥24×24 via spacing, not padding, to keep density |
| 21 | missing keyboard shortcuts | the §1.2 table plus a `Cmd+/` sheet |
| 22 | `alert()` / `confirm()` | native dialog for real decisions; toast+undo for reversible ones |
| 23 | no tray / deep-link / single-instance | `single-instance` first, `deep-link` for `peregrine://`, optional tray |
| 24 | `Cmd+W` quitting the app | close hides on macOS, quit only on `Cmd+Q`, dock click reopens |
| 25 | infinite toast stack | max 3 concurrent, 5s informative, 8s undoable, pause on hover |
| 26 | scroll position reset on back | persist rail offsets in history state, restore on `popstate` |
| 27 | ignoring reduced-motion / forced-colors | explicit branches; test with the OS toggles on |
| 28 | autoplaying video with sound on hover | muted, no audio track, ≤10s loop, never on keyboard focus |
| 29 | disabling buttons during in-flight mutations | optimistic UI; disable only on terminal states |
| 30 | 12px grey-on-grey body text | 14px body minimum, 4.5:1 enforced by the token validator |

**Recommendation:** adopt as a PR checklist, with automated gates for 4, 11, 15, 19, 20, 27, 30 —
all statically detectable in CSS and tokens.

---

## 5. Theme system

**Modes.** `light`, `dark`, `system` (default), `custom`. Resolve `system` with
`matchMedia('(prefers-color-scheme: dark)')` plus its `change` event; persist the choice, not the
resolved value. Mica and macOS vibrancy follow the OS appearance; on Linux honour
`gtk-application-prefer-dark-theme` / `color-scheme`.

| Platform | Accent source | CSS hook | Fallback |
|---|---|---|---|
| macOS | system accent / highlight colour | `-apple-system-control-accent` | `AccentColor`, then a tuned OKLCH brand hue |
| Windows 11 | Personalization ▸ Colours | `AccentColor` / `AccentColorText` | same |
| GNOME/libadwaita | libadwaita ≥1.6 accent | not exposed to the webview; read via a small Rust command | same |

Never trust the emitted accent for text contrast: derive `--accent-text` by checking OKLCH lightness,
flipping white/black around ~0.62 L, then validating the pair.

**Tokens — three strict layers.** Primitive (`--c-50…--c-900`, one hue) → semantic (`--surface-0/1/2`,
`--text-primary/secondary`, `--border-subtle`, `--accent`, `--danger`, `--focus-ring`) → component.
Components must never reference primitives; lint it.

```css
:root { --hue-brand: 265;
  --surface-0: oklch(0.99 0.002 var(--hue-brand));
  --surface-1: oklch(0.97 0.004 var(--hue-brand));
  --text-primary:   oklch(0.20 0.01 var(--hue-brand));
  --text-secondary: oklch(0.45 0.01 var(--hue-brand));   /* must clear 4.5:1 */
  --border-subtle:  oklch(0.90 0.006 var(--hue-brand));
  --accent: oklch(0.62 0.19 var(--hue-brand)); --accent-text: oklch(0.99 0 0);
  --focus-ring: var(--accent); }
[data-theme="dark"] { --surface-0: oklch(0.20 0.008 var(--hue-brand));
  --surface-1: oklch(0.24 0.010 var(--hue-brand));
  --text-primary: oklch(0.96 0.005 var(--hue-brand));
  --text-secondary: oklch(0.78 0.010 var(--hue-brand));
  --border-subtle: oklch(0.34 0.010 var(--hue-brand)); }
```

OKLCH's perceptually uniform lightness means one `L` shift holds contrast across hues, which is what
makes automated validation tractable. Custom themes expose exactly three inputs: `--hue-brand`,
`--chroma-scale` (0.6–1.4), and a surface tint (L 0.97–0.995 light / 0.16–0.24 dark).

```css
@media (prefers-contrast: more) { :root { --text-secondary: var(--text-primary); --border-subtle: var(--text-primary); } }
@media (prefers-reduced-transparency: reduce) { .vibrancy { backdrop-filter: none; background: var(--surface-1); } }
@media (forced-colors: active) { .card { forced-color-adjust: none; border: 1px solid CanvasText; } }
```

Mica has documented solid fallbacks (`SolidBackgroundFillColorBase`/`…Alt`) when transparency is off,
Battery Saver is on, hardware is low-end, the window deactivates, or the build is <22000 — mirror that
by making Mica a backdrop *behind* an opaque `--surface-0`, so a missing effect is invisible.

**Contrast guarantees.** WCAG 2.2 SC 1.4.3 AA: ≥4.5:1 text, ≥3:1 large text (≥18.66px bold or ≥24px);
SC 1.4.11: ≥3:1 for UI components and graphics including scrollbar thumbs; focus indicators ≥3:1 and
≥2 CSS px. APCA is informative only — |Lc| ≥75 body, ≥60 large and media-adjacent, never APCA-only
where WCAG fails. Enforce in CI: a Node script enumerates every real semantic pair
(`text-primary × surface-0/1/2`, `text-secondary × surface-0/1`, `accent × accent-text`,
`border × surface`, focus ring × surface), converts OKLCH → sRGB → relative luminance, asserts the
ratio, and fails the build; the runtime custom-theme generator fails too, so a user cannot author an
inaccessible theme. Text is never placed directly on media — use a scrim and validate against its
darkest point.

**Recommendation:** OKLCH three-layer tokens consumed only semantically; `system` default with a
persisted explicit choice; OS accent where exposed, tuned brand hue otherwise; explicit
`prefers-contrast`, `forced-colors`, and reduced-transparency branches; a CI contrast validator gating
every semantic pair on WCAG 2.2 AA with APCA as a secondary signal.

---

## 6. Sources

Tauri v2: [config](https://v2.tauri.app/reference/config/) · [window customization](https://v2.tauri.app/learn/window-customization/) · [Linux graphics](https://v2.tauri.app/develop/debug/linux-graphics/) · [window-state](https://v2.tauri.app/plugin/window-state/) · [single-instance](https://v2.tauri.app/plugin/single-instance/) · [deep-link](https://v2.tauri.app/plugin/deep-linking/) · [notification](https://v2.tauri.app/plugin/notification/) · [overlay drag bug](https://github.com/tauri-apps/tauri/issues/4316).
Microsoft: [title bar design](https://learn.microsoft.com/en-us/windows/apps/design/basics/titlebar-design) · [title bar dev](https://learn.microsoft.com/en-us/windows/apps/develop/title-bar) · [Mica](https://learn.microsoft.com/en-us/windows/apps/design/style/mica) · [type ramp](https://learn.microsoft.com/en-us/windows/apps/design/signature-experiences/typography).
Motion/spec: [M3 token source](https://github.com/androidx/androidx/blob/androidx-main/compose/material3/material3/src/commonMain/kotlin/androidx/compose/material3/tokens/MotionTokens.kt) · [M3 easing & duration](https://m3.material.io/styles/motion/easing-and-duration/tokens-specs) · [Apple fluid interfaces WWDC18](https://developer.apple.com/videos/play/wwdc2018/803/) · [Apple HIG](https://developer.apple.com/design/human-interface-guidelines/) · [scroll-driven animations](https://drafts.csswg.org/scroll-animations-1/).
Linux: [GNOME typography](https://developer.gnome.org/hig/guidelines/typography.html) · [header bars](https://developer.gnome.org/hig/patterns/containers/header-bars.html) · [UI styling](https://developer.gnome.org/hig/guidelines/ui-styling.html) · [keyboard shortcuts](https://developer.gnome.org/hig/reference/keyboard.html) · [KDE HIG](https://develop.kde.org/hig/) · [WebKitGTK settings](https://webkitgtk.org/reference/webkit2gtk/stable/).
Web/a11y: [SC 1.4.3](https://www.w3.org/WAI/WCAG22/Understanding/contrast-minimum.html) · [SC 2.5.8](https://www.w3.org/WAI/WCAG22/Understanding/target-size-minimum.html) · [content-visibility](https://developer.mozilla.org/en-US/docs/Web/CSS/content-visibility) · [will-change](https://developer.mozilla.org/en-US/docs/Web/CSS/will-change) · [overscroll-behavior](https://developer.mozilla.org/en-US/docs/Web/CSS/overscroll-behavior) · [prefers-reduced-motion](https://developer.mozilla.org/en-US/docs/Web/CSS/@media/prefers-reduced-motion) · [prefers-contrast](https://developer.mozilla.org/en-US/docs/Web/CSS/@media/prefers-contrast) · [forced-colors](https://developer.mozilla.org/en-US/docs/Web/CSS/@media/forced-colors) · [View Transition API](https://developer.mozilla.org/en-US/docs/Web/API/View_Transition_API) · [fetchPriority](https://developer.mozilla.org/en-US/docs/Web/API/HTMLImageElement/fetchPriority).
