import type { IconName } from "@peregrine/ui";

/**
 * The screens this build has, in the order the sidebar lists them
 * (docs/ARCHITECTURE.md §6, "Routing map").
 *
 * A screen appears here only once it exists. The sidebar is the app's navigation and the
 * shell's only way of reaching a screen, so an entry that opens nothing would be a control
 * that lies; Browse, Search, Title detail, Downloads, Player, and Cast join the list with the
 * tasks that build them (PG-E7, PG-E13, PG-E16). Diagnostics is here rather
 * than in Settings because `crate::menu` and the shell both reach it directly today.
 */
export type Screen = "discover" | "library" | "jobs" | "settings";

export interface ScreenEntry {
  readonly id: Screen;
  /** The catalogue key, used for the sidebar row and the screen's own title. */
  readonly labelKey: string;
  readonly icon: IconName;
}

export const SCREENS: readonly ScreenEntry[] = [
  { id: "discover", labelKey: "discover.title", icon: "compass" },
  // Second, because it is the one screen that works without a provider: what is on this machine is
  // known as soon as a scan has run, while Discover waits on the metadata road.
  { id: "library", labelKey: "library.title", icon: "library" },
  { id: "jobs", labelKey: "jobs.title", icon: "pulse" },
  { id: "settings", labelKey: "settings.title", icon: "tune" },
];

/**
 * The screen a registered shortcut opens, or `undefined` for an action this shell has no
 * screen for.
 *
 * The mapping lives here rather than in the core's registry because it is routing: the
 * core knows an action's id and nothing about which component shows it (AGENTS R7). An id
 * with no screen is ignored rather than guessed at, so a registry entry added before its
 * screen appears does nothing instead of opening the wrong one.
 */
export const SCREEN_FOR_ACTION: Readonly<Record<string, Screen>> = {
  "navigation.discover": "discover",
  "navigation.jobs": "jobs",
  "navigation.settings": "settings",
};
