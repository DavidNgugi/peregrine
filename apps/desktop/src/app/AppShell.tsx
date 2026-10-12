import { NowPlayingBar } from "@peregrine/player-ui";
import { Button } from "@peregrine/ui";
import type { ReactElement } from "react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { DiscoverScreen } from "../features/discover/index.js";
import { JobsScreen, useJobsBridge } from "../features/jobs/index.js";
import type { Platform } from "../lib/platform.js";
import { useTheme } from "./useTheme.js";
import { WindowChrome } from "./WindowChrome.js";

/** Which screen the content area shows. */
type Screen = "discover" | "jobs";

interface AppShellProps {
  /** Which platform's window chrome to present; `null` when the core could not be asked. */
  readonly platform: Platform | null;
}

/**
 * The chrome that persists across navigation: the window chrome, the current screen, and
 * the player bar (docs/ARCHITECTURE.md §6 rule 6).
 *
 * The screen switch is temporary. Typed routing arrives with PG-E7-02 and the sidebar with
 * PG-E7-01; until then the header exposes the two screens that exist, because the
 * diagnostics surface has to be reachable to be a deliverable at all (PLAN §8 item 1).
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
        {screen === "discover" ? <DiscoverScreen /> : <JobsScreen />}
      </main>
      <NowPlayingBar idleLabel={t("shell.nowPlaying.idle")} state={{ kind: "idle" }} />
    </div>
  );
}
