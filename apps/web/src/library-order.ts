/**
 * The library grid's order: the most recently added title first.
 *
 * The core's read is keyset-paged by title id, which is stable but not the question this screen
 * asks. `addedAt` is already the newest file or item time the read aggregates, so ordering on it
 * here changes only the drawing; the next page's cursor stays the core's.
 */

/** A row with the two fields the order reads. */
export interface RecentlyAdded {
  readonly id: number;
  readonly addedAt: number;
}

/** Whether a row's spelling of its kind matches the library tab. */
// The core spells a kind `movie`/`show`; the desktop's library DTO spells it `movies`/`shows`.
// Accepting both is the difference between a tab that filters and one that hides every row.
export function matchesKind(rowKind: string, filter: "all" | "movie" | "show"): boolean {
  return filter === "all" || rowKind === filter || rowKind === filter + "s";
}

/** The rows, newest addition first; the id breaks a same-millisecond tie. */
export function byRecentlyAdded<T extends RecentlyAdded>(rows: readonly T[]): T[] {
  return [...rows].sort((left, right) => right.addedAt - left.addedAt || right.id - left.id);
}
