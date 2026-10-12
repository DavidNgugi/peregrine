import { NowPlayingBar } from "@peregrine/player-ui";
import { IconButton, Sidebar, SidebarItem } from "@peregrine/ui";
import type { ReactElement } from "react";
import { useCallback, useState } from "react";
import { useTranslation } from "react-i18next";
import { DiscoverScreen } from "../features/discover/index.js";
import { JobsScreen, useJobsBridge } from "../features/jobs/index.js";
import {
  SettingsScreen,
  useLibraryRoots,
  useShortcutDispatch,
  useShortcuts,
} from "../features/settings/index.js";
import type { Platform } from "../lib/platform.js";
import { SCREENS, SCREEN_FOR_ACTION, type Screen } from "./screens.js";
import { useTheme } from "./useTheme.js";
import { WindowChrome } from "./WindowChrome.js";

interface AppShellProps {
  /** Which platform's window chrome to present; `null` when the core could not be asked. */
  readonly platform: Platform | null;
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
 * The screen switch is still state rather than a route: typed routing is PG-E7-02, and the
 * sidebar rows become links when it lands.
 */
export function AppShell({ platform }: AppShellProps): ReactElement {
  const { t } = useTranslation();
  const { resolved, setPreference } = useTheme();
  const [screen, setScreen] = useState<Screen>("discover");

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
  const openScreen = useCallback((action: string) => {
    const target = SCREEN_FOR_ACTION[action];
    if (target !== undefined) {
      setScreen(target);
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
              current={entry.id === screen}
              icon={entry.icon}
              key={entry.id}
              label={t(entry.labelKey)}
              onSelect={() => {
                setScreen(entry.id);
              }}
            />
          ))}
        </Sidebar>
        <main className="min-h-0 min-w-0 flex-1 overflow-hidden">
          {screen === "discover" ? <DiscoverScreen /> : null}
          {screen === "jobs" ? <JobsScreen /> : null}
          {screen === "settings" ? (
            <SettingsScreen libraries={libraries} shortcuts={shortcuts} />
          ) : null}
        </main>
      </div>
      <NowPlayingBar idleLabel={t("shell.nowPlaying.idle")} state={{ kind: "idle" }} />
    </div>
  );
}
