import { NowPlayingBar } from "@peregrine/player-ui";
import { IconButton, Sidebar, SidebarItem } from "@peregrine/ui";
import type { ReactElement } from "react";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { DiscoverScreen } from "../features/discover/index.js";
import { JobsScreen, useJobsBridge } from "../features/jobs/index.js";
import { TitleScreen, useTitle } from "../features/title/index.js";
import {
  LibraryScreen,
  useLibraryTitles,
  useMatchRules,
  useScan,
  useUnmatchedFiles,
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
import { useTheme } from "./useTheme.js";
import { WindowChrome } from "./WindowChrome.js";

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

  // Mounted by the shell rather than by the screen so a running job survives navigation.
  useJobsBridge();

  // The registry is the core's (PG-E1-08); the same answer drives the Settings list and
  // the keys this shell acts on, so the two cannot disagree.
  const shortcuts = useShortcuts();
  const libraries = useLibraryRoots();
  const providers = useProviders();
  const library = useLibraryTitles();
  const scan = useScan();
  const unmatched = useUnmatchedFiles();
  const rules = useMatchRules();
  // One title's screen is the only route with a parameter. The hook is always called — the rules of
  // hooks — and reads nothing when the route names no title.
  const title = useTitle(route.kind === "title" ? route.id : null);
  const openScreen = useCallback((action: string) => {
    const target = ROUTE_FOR_ACTION[action];
    if (target !== undefined) {
      setRoute(target);
    }
  }, []);
  useShortcutDispatch(shortcuts.bindings, openScreen);

  return (
    <div className="flex h-full flex-col bg-surface-0 text-text-primary">
      <WindowChrome platform={platform}>
        <IconButton
          icon={nextTheme === "light" ? "sun" : "moon"}
          label={themeLabel}
          onClick={() => {
            setPreference(nextTheme);
          }}
          size="sm"
        />
      </WindowChrome>
      <div className="flex min-h-0 flex-1">
        <Sidebar label={t("shell.nav.label")}>
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
        <main className="min-h-0 min-w-0 flex-1 overflow-hidden">
          {route.kind === "discover" ? <DiscoverScreen onNavigate={setRoute} /> : null}
          {route.kind === "library" ? (
            <LibraryScreen
              onNavigate={setRoute}
              rules={rules}
              scan={scan}
              titles={library}
              unmatched={unmatched}
            />
          ) : null}
          {route.kind === "diagnostics" ? <JobsScreen /> : null}
          {route.kind === "settings" ? (
            <SettingsScreen
              libraries={libraries}
              providers={providers}
              shortcuts={shortcuts}
            />
          ) : null}
          {route.kind === "title" ? <TitleScreen detail={title} kind={route.titleKind} /> : null}
        </main>
      </div>
      <NowPlayingBar idleLabel={t("shell.nowPlaying.idle")} state={{ kind: "idle" }} />
    </div>
  );
}
