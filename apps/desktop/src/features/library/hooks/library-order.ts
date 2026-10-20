/**
 * The library's display order: most recently added first.
 *
 * The core returns a keyset page ordered by title id, which is stable but not what a reader means
 * by "my library". `addedAt` is the newest file or item time the row aggregates, so sorting on it
 * changes the drawing without changing the read or its cursor.
 */

/** The two fields the order reads. */
export interface RecentlyAdded {
  readonly id: number;
  readonly addedAt: number;
}

/** The rows, newest addition first; the id breaks a same-millisecond tie. */
export function byRecentlyAdded<T extends RecentlyAdded>(rows: readonly T[]): T[] {
  return [...rows].sort((left, right) => right.addedAt - left.addedAt || right.id - left.id);
}
