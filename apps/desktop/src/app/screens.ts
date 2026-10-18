import type { IconName } from "@peregrine/ui";

import type { Route } from "./routes.js";

/**
 * The screens this build has, in the order the sidebar lists them
 * (docs/ARCHITECTURE.md §6, "Routing map").
 *
 * A screen appears here only once it exists. The sidebar is the app's navigation and the
 * shell's only way of reaching a screen, so an entry that opens nothing would be a control
 * that lies; Search, Title detail, Downloads, Player, and Cast join the list with the
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
  // Second, because it is the screen Discover cannot be: a rail is a fixed list a provider
  // publishes, and this is the queried grid a person narrows — by kind, genre, score, and order.
  { route: { kind: "browse" }, labelKey: "browse.title", icon: "grid" },
  // After the two catalogue reads, because it is the one screen that works without a provider: what
  // is on this machine is known as soon as a scan has run, while the two above wait on the metadata
  // road.
  { route: { kind: "library" }, labelKey: "library.title", icon: "library" },
  // After the library, because a download is one of the two ways a title reaches it: what the
  // scanner found on disk, and what the acquisition service is fetching.
  { route: { kind: "downloads" }, labelKey: "downloads.title", icon: "download" },
  // What has been watched is personal history rather than a catalogue: it belongs beside the
  // queue, not inside Settings, where two hundred rows would bury every other control.
  { route: { kind: "history" }, labelKey: "history.title", icon: "history" },
  { route: { kind: "diagnostics" }, labelKey: "jobs.title", icon: "pulse" },
  { route: { kind: "settings" }, labelKey: "settings.title", icon: "tune" },
];
