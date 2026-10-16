import { NowPlayingBar } from "@peregrine/player-ui";
import { IconButton, Sidebar, SidebarItem } from "@peregrine/ui";
import type { ReactElement } from "react";
import { useCallback, useEffect, useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { BrowseScreen } from "../features/browse/index.js";
import { DiscoverScreen } from "../features/discover/index.js";
import { DownloadsScreen, useDownloads } from "../features/downloads/index.js";
import { JobsScreen, useJobsBridge } from "../features/jobs/index.js";
import { PlayerScreen } from "../features/player/index.js";
import {
  TitleScreen,
  useAcquire,
  useSeasons,
  useSimilarTitles,
  useTitle,
  useTitleFeed,
} from "../features/title/index.js";
import {
  LibraryScreen,
  useLibraryTitles,
  useMatchRules,
  useScan,
  useUnmatchedFiles,
  useLibraryView,
} from "../features/library/index.js";
import {
  SettingsScreen,
  useLibraryRoots,
  useProviders,
  useShortcutDispatch,
  useShortcuts,
} from "../features/settings/index.js";
import type { Platform } from "../lib/platform.js";
import { writePath } from "../lib/window-path.js";
import { ROUTE_FOR_ACTION, routePath, type Route } from "./routes.js";
import { SCREENS } from "./screens.js";
import { useFullscreen } from "./useFullscreen.js";
import { useTheme } from "./useTheme.js";

interface AppShellProps {
  /** Which platform's window chrome to present; `null` when the core could not be asked. */
  readonly platform: Platform | null;
  /**
   * The route the window opened at.
   *
   * A prop rather than a read of `window.location`, so that the shell — the thing its tests render —
   * does not need a browser to exist. The entry that mounts it reads the path.
   */
  readonly route: Route;
}

/**
 * The chrome that persists across navigation: the window chrome, the sidebar, the current
 * screen, and the player bar (docs/ARCHITECTURE.md §6 rule 6, docs/DESIGN_SYSTEM.md §6).
 *
 * Only the content area is swapped when the screen changes. The chrome sits above the
 * sidebar and the player bar below it, both outside the element the screen renders into, so
 * a screen that mounts or unmounts cannot take the navigation or the player with it — which
 * is the property PG-E7-01's acceptance names, and the reason the screen switch is a
 * ternary inside one `main` rather than a component that replaces the whole tree.
 *
 * The screen switch is a route (`./routes.js`) rather than a name: the sidebar, the shortcut
 * dispatcher, and the path the window opened at all speak the same type, so a place in the
 * application is spelled once. What is still owed by PG-E7-02 is the other half of its acceptance —
 * cache-first loaders — and the routes for screens that do not exist yet; the old note about typed
 * routing living here is the part that has landed.
 * sidebar rows become links when it lands.
 */
export function AppShell({ platform, route: initialRoute }: AppShellProps): ReactElement {
  const { t } = useTranslation();
  const { resolved, setPreference } = useTheme();
  // Where the user is, which the sidebar, the shortcut dispatcher, and the screen switch all read
  // from one value. It starts where the window opened and only this shell moves it.
  const [route, setRoute] = useState<Route>(initialRoute);

  // And the window's path follows it, so the two cannot drift: a reload — which a webview performs
  // for reasons it does not announce — returns the user to the screen they were on rather than to
  // the one the window happened to open at.
  useEffect(() => {
    writePath(routePath(route));
  }, [route]);

  // The icon and the name describe the same action — what the button would switch *to* —
  // so a sighted user and a screen reader are told the same thing.
  const nextTheme = resolved === "dark" ? "light" : "dark";
  const themeLabel = t(
    nextTheme === "light" ? "shell.theme.switchToLight" : "shell.theme.switchToDark",
  );

  // Whether the sidebar shows its labels, which is the user's own answer and not the window's width:
  // the width rule is the sidebar's. Nothing persists it yet — the store that would is PG-E1-02, and
  // the shortcut that would toggle it is PG-E21-05's registry entry.
  const [collapsed, setCollapsed] = useState(false);
  const fullscreen = useFullscreen();
  const sidebarId = useId();
  const collapseLabel = collapsed ? t("shell.nav.expand") : t("shell.nav.collapse");

  // **The window's top-left is the sidebar's**, because there is no title bar of our own (§8 rules a
  // fake one out). A macOS window in the `Overlay` style draws the traffic lights over this column, so
  // the strip leaves their band empty and puts the toggle *below* them — beside them there is no room
  // (the cluster is ~70 px wide and a collapsed rail is 60), and a control underneath them cannot be
  // clicked. That band is also the drag region: with no bar, this is where a person grabs the window.
  //
  // **In every other case the toggle sits level with the search field beside it**, which is a macOS
  // full screen (the lights hide until the pointer finds the top edge), Windows, and Linux. `22 px` is
  // the arithmetic and not a token: the hero's toolbar pads by 16, its field is 44 tall, so the
  // *marks* line up when a 20 px mark inside this 32 px button starts at 16 + (44 − 20) / 2 − (32 − 20)
  // / 2 = 22. Aligning the boxes instead would leave the two marks 6 px apart, which is the thing an
  // eye catches.
  //
  // **The toggle is a `SidebarItem` that is not a row**, and it is laid out as one so its mark lands
  // on the same x as the marks below it. A row is 32 px tall with a 20 px mark at its own 8 px padding
  // edge — 16 px from the column's edge — while the toggle centres a 20 px mark in a 32 px button, so
  // its box has to start at 10: `p-2` plus the 2 px below. A collapsed rail centres its rows instead
  // (their marks sit at 20), so `justify-center` takes over and the same box lands there.
  const sidebarHeader = (
    <div
      className={[
        "flex shrink-0 items-center p-2",
        platform === "macos" && !fullscreen ? "pt-[36px]" : "pt-[22px]",
        collapsed ? "justify-center" : "",
      ].join(" ")}
      data-tauri-drag-region={platform === "macos" ? "deep" : "false"}
    >
      <IconButton
        aria-controls={sidebarId}
        aria-expanded={!collapsed}
        className="ms-[2px]"
        icon="sidebar"
        label={collapseLabel}
        onClick={() => {
          setCollapsed((current) => !current);
        }}
        title={collapseLabel}
      />
    </div>
  );

  // Mounted by the shell rather than by the screen so a running job survives navigation.
  useJobsBridge();

  // The registry is the core's (PG-E1-08); the same answer drives the Settings list and
  // the keys this shell acts on, so the two cannot disagree.
  const shortcuts = useShortcuts();
  const libraries = useLibraryRoots();
  const providers = useProviders();
  const library = useLibraryTitles();
  const downloads = useDownloads();
  const scan = useScan();
  const unmatched = useUnmatchedFiles();
  const rules = useMatchRules();
  // Which drawing the library screen uses. Held here rather than in the screen because `main` renders
  // one screen at a time, so a screen's own state does not survive a visit to a title and back.
  const libraryView = useLibraryView();
  // One title's screen is the only route with a parameter. The hook is always called — the rules of
  // hooks — and reads nothing when the route names no title.
  const acquire = useAcquire();
  const title = useTitle(route.kind === "title" ? route.id : null);
  // The recommendations the page draws under its episodes, asked for by the same rule and for the
  // same route: a provider read that the page can live without, which is why it is its own hook.
  const similar = useSimilarTitles(route.kind === "title" ? route.id : null);
  // **The feed the page's one list draws from**, built here with the other title reads so the screen
  // stays a projection: the recommendations are its head, and the genres they share are the queue of
  // pages behind them. One list and one scroll area, where the first shape was a rail with a shelf per
  // genre under it.
  const feed = useTitleFeed(route.kind === "title" ? route.titleKind : "movie", similar);
  // The seasons and the season on screen, asked for by the same route. A film is answered without a
  // request, which is the same short circuit the title read makes.
  const seasons = useSeasons(
    route.kind === "title" ? route.id : null,
    route.kind === "title" ? route.titleKind : "movie",
  );
  const openScreen = useCallback((action: string) => {
    const target = ROUTE_FOR_ACTION[action];
    if (target !== undefined) {
      setRoute(target);
    }
  }, []);
  useShortcutDispatch(shortcuts.bindings, openScreen);

  return (
    <div className="flex h-full flex-col bg-surface-0 text-text-primary">
      <div className="flex min-h-0 flex-1">
        <Sidebar
          collapsed={collapsed}
          header={sidebarHeader}
          id={sidebarId}
          label={t("shell.nav.label")}
        >
          {SCREENS.map((entry) => (
            <SidebarItem
              current={entry.route.kind === route.kind}
              icon={entry.icon}
              key={entry.route.kind}
              label={t(entry.labelKey)}
              onSelect={() => {
                setRoute(entry.route);
              }}
            />
          ))}
        </Sidebar>
        {/* `deep` makes the whole column draggable except the controls inside it — Tauri's drag
            handler walks the event's path and skips anything clickable — which is what gives a window
            with no title bar somewhere to be grabbed besides the sidebar's strip. */}
        {/* No drag region here, deliberately: the chrome's is the sidebar's strip, and a screen's own
            area carrying one would let a screen take the window's behaviour with it — which is the
            property `App.test.tsx` pins. On macOS the overlay title bar drags on its own; on Windows
            and Linux the frame and its caption do. */}
        <main className="min-h-0 min-w-0 flex-1 overflow-hidden">
          {route.kind === "discover" ? (
            /* A bare `/` is All, which is the default the toolbar starts on and what `parseRoute`
               answers when the URL carries no `type`. */
            <DiscoverScreen
              onNavigate={setRoute}
              titleKind={route.titleKind}
              trailing={
                /* The theme control, which used to live in the title bar. Same shape as the toolbar's
                   own icon buttons — 44 px, on the translucent surface — because it sits beside them:
                   the refresh control that follows it is the neighbour it has to match. */
                <IconButton
                  className="size-11 rounded-full border border-border-subtle bg-surface-0/70"
                  icon={nextTheme === "light" ? "sun" : "moon"}
                  label={themeLabel}
                  onClick={() => {
                    setPreference(nextTheme);
                  }}
                  title={themeLabel}
                  variant="ghost"
                />
              }
            />
          ) : null}
          {route.kind === "browse" ? (
            /* The kind is the route's, so the toolbar's switch is a navigation and the window's
               path says which category is on screen. A route naming none is the default kind — see
               `BrowseScreen`. */
            <BrowseScreen onNavigate={setRoute} titleKind={route.titleKind} />
          ) : null}
          {route.kind === "library" ? (
            <LibraryScreen
              onNavigate={setRoute}
              onView={libraryView.setView}
              rules={rules}
              scan={scan}
              titles={library}
              unmatched={unmatched}
              view={libraryView.view}
            />
          ) : null}
          {route.kind === "downloads" ? <DownloadsScreen downloads={downloads} /> : null}
          {route.kind === "diagnostics" ? <JobsScreen /> : null}
          {route.kind === "settings" ? (
            <SettingsScreen libraries={libraries} providers={providers} shortcuts={shortcuts} />
          ) : null}
          {route.kind === "player" ? (
            /* The player is a screen of its own rather than a state inside the title page: the video
               wants the content area, the title page's counts and seasons want to stay where they are,
               and a route is what makes Back mean what it says. The session it opens lives exactly as
               long as this component does (ADR 0023, SECURITY.md T10). */
            <PlayerScreen
              id={route.id}
              onClose={() => {
                setRoute({ kind: "title", titleKind: "movie", id: route.id });
              }}
            />
          ) : null}
          {route.kind === "title" ? (
            <TitleScreen
              detail={title}
              feed={feed}
              kind={route.titleKind}
              /* **Keyed by which title it is, and its absence was a bug a reader hit.** Opening a
                 title from another title's feed is the same component in the same position, so React
                 reconciled it onto the *same* `<section>` — and that section is the screen's scroller.
                 The new page therefore opened at the old page's scroll offset, so following a
                 recommendation looked like nothing had happened: the reader stayed part-way down a
                 different title and had to scroll up to find its name. Changing the key remounts the
                 screen, which resets the scroll and, just as importantly, drops the previous title's
                 component state — the season switcher would otherwise open on the season the *last*
                 title was left on. */
              key={`${route.titleKind}:${String(route.id)}`}
              acquire={acquire}
              onNavigate={setRoute}
              seasons={seasons}
            />
          ) : null}
        </main>
      </div>
      <NowPlayingBar idleLabel={t("shell.nowPlaying.idle")} state={{ kind: "idle" }} />
    </div>
  );
}
