import { NowPlayingBar } from "@peregrine/player-ui";
import { Button } from "@peregrine/ui";
import type { ReactElement } from "react";
import { useCallback, useState } from "react";
import { useTranslation } from "react-i18next";
import { DiscoverScreen } from "../features/discover/index.js";
import { JobsScreen, useJobsBridge } from "../features/jobs/index.js";
import { SettingsScreen, useShortcutDispatch, useShortcuts } from "../features/settings/index.js";
import type { Platform } from "../lib/platform.js";
import { useTheme } from "./useTheme.js";
import { WindowChrome } from "./WindowChrome.js";

/** Which screen the content area shows. */
type Screen = "discover" | "jobs" | "settings";

/**
 * The screen a registered shortcut opens, or `null` for an action this shell has no
 * screen for.
 *
 * The mapping lives here rather than in the registry because it is routing: the core
 * knows an action's id and nothing about which component shows it (AGENTS R7). An id
 * with no screen is ignored rather than guessed at, so a registry entry added before its
 * screen appears does nothing instead of opening the wrong one.
 */
const SCREEN_FOR_ACTION: Readonly<Record<string, Screen>> = {
  "navigation.discover": "discover",
  "navigation.jobs": "jobs",
  "navigation.settings": "settings",
};

interface AppShellProps {
  /** Which platform's window chrome to present; `null` when the core could not be asked. */
  readonly platform: Platform | null;
}

/**
 * The chrome that persists across navigation: the window chrome, the current screen, and
 * the player bar (docs/ARCHITECTURE.md §6 rule 6).
 *
 * The screen switch is temporary. Typed routing arrives with PG-E7-02 and the sidebar with
 * PG-E7-01; until then the header exposes the screens that exist, because the diagnostics
 * and settings surfaces have to be reachable to be deliverables at all (PLAN §8 item 1).
 */
export function AppShell({ platform }: AppShellProps): ReactElement {
  const { t } = useTranslation();
  const { resolved, setPreference } = useTheme();
  const [screen, setScreen] = useState<Screen>("discover");
  const nextTheme = resolved === "dark" ? "light" : "dark";
  const themeLabel = t(
    nextTheme === "light" ? "shell.theme.switchToLight" : "shell.theme.switchToDark",
  );

  // Mounted by the shell rather than by the screen so a running job survives navigation.
  useJobsBridge();

  // The registry is the core's (PG-E1-08); the same answer drives the Settings list and
  // the keys this shell acts on, so the two cannot disagree.
  const shortcuts = useShortcuts();
  const openScreen = useCallback((action: string) => {
    const target = SCREEN_FOR_ACTION[action];
    if (target !== undefined) {
      setScreen(target);
    }
  }, []);
  useShortcutDispatch(shortcuts, openScreen);

  return (
    <div className="flex h-full flex-col bg-surface-0 text-text-primary">
      <WindowChrome platform={platform}>
        <nav aria-label={t("shell.nav.label")} className="flex items-center gap-1">
          <Button
            aria-current={screen === "discover" ? "page" : undefined}
            onClick={() => {
              setScreen("discover");
            }}
            size="sm"
            variant={screen === "discover" ? "secondary" : "ghost"}
          >
            {t("discover.title")}
          </Button>
          <Button
            aria-current={screen === "jobs" ? "page" : undefined}
            onClick={() => {
              setScreen("jobs");
            }}
            size="sm"
            variant={screen === "jobs" ? "secondary" : "ghost"}
          >
            {t("jobs.title")}
          </Button>
          <Button
            aria-current={screen === "settings" ? "page" : undefined}
            onClick={() => {
              setScreen("settings");
            }}
            size="sm"
            variant={screen === "settings" ? "secondary" : "ghost"}
          >
            {t("settings.title")}
          </Button>
          <Button
            aria-label={themeLabel}
            onClick={() => {
              setPreference(nextTheme);
            }}
            size="sm"
            variant="ghost"
          >
            {themeLabel}
          </Button>
        </nav>
      </WindowChrome>
      <main className="min-h-0 flex-1 overflow-hidden">
        {screen === "discover" ? <DiscoverScreen /> : null}
        {screen === "jobs" ? <JobsScreen /> : null}
        {screen === "settings" ? <SettingsScreen bindings={shortcuts} /> : null}
      </main>
      <NowPlayingBar idleLabel={t("shell.nowPlaying.idle")} state={{ kind: "idle" }} />
    </div>
  );
}
