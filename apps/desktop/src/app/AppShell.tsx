import { NowPlayingBar } from "@peregrine/player-ui";
import { Button } from "@peregrine/ui";
import type { ReactElement } from "react";
import { useTranslation } from "react-i18next";
import { DiscoverScreen } from "../features/discover/index.js";
import { useTheme } from "./useTheme.js";

/**
 * The chrome that persists across navigation: header, the current screen, and the
 * player bar (docs/ARCHITECTURE.md §6 rule 6). Routing arrives with PG-E7-01, so
 * Discover is the only screen mounted today.
 */
export function AppShell(): ReactElement {
  const { t } = useTranslation();
  const { resolved, setPreference } = useTheme();
  const nextTheme = resolved === "dark" ? "light" : "dark";
  const themeLabel = t(
    nextTheme === "light" ? "shell.theme.switchToLight" : "shell.theme.switchToDark",
  );

  return (
    <div className="flex h-full flex-col bg-surface-0 text-text-primary">
      <header className="flex h-12 shrink-0 items-center justify-between border-b border-border-subtle px-3">
        <span className="text-title-2">{t("app.name")}</span>
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
      </header>
      <main className="min-h-0 flex-1 overflow-hidden">
        <DiscoverScreen />
      </main>
      <NowPlayingBar idleLabel={t("shell.nowPlaying.idle")} state={{ kind: "idle" }} />
    </div>
  );
}
