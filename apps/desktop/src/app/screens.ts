import type { IconName } from "@peregrine/ui";

import type { Route } from "./routes.js";

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
export interface ScreenEntry {
  /** Where the sidebar row opens, which is also what tells it whether it is current. */
  readonly route: Route;
  /** The catalogue key, used for the sidebar row and the screen's own title. */
  readonly labelKey: string;
  readonly icon: IconName;
}

export const SCREENS: readonly ScreenEntry[] = [
  { route: { kind: "discover" }, labelKey: "discover.title", icon: "compass" },
  // Second, because it is the one screen that works without a provider: what is on this machine is
  // known as soon as a scan has run, while Discover waits on the metadata road.
  { route: { kind: "library" }, labelKey: "library.title", icon: "library" },
  { route: { kind: "diagnostics" }, labelKey: "jobs.title", icon: "pulse" },
  { route: { kind: "settings" }, labelKey: "settings.title", icon: "tune" },
];
