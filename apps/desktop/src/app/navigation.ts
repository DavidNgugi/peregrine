/**
 * The shell's navigation history: where the user has been, and how they get back.
 *
 * **The stack is the app's, not the browser's.** The window's path is kept in step with the screen
 * ({@link ./window-path.js}) so a reload returns to where the user was, but it is *replaced* rather
 * than pushed: a webview's own back stack would also walk through the sidebar rows and the
 * shortcut jumps, and this is the one place that decides what Back means. Keeping the history out of
 * the browser also keeps it testable without one — the reducer below is pure, and the shell's Node
 * tests render it with no `window` at all.
 *
 * **Forward is dropped on a new move, which is what makes the stack a history rather than a list.**
 * Going back and then opening somewhere else discards the places the user had ahead of them, the
 * same way a browser does; keeping them would make Back mean "the last thing I visited in any
 * direction", which is not a history. The one exception is a move to where the user already is:
 * pressing the current sidebar row is not a visit, and pushing it would make Back do nothing
 * visible.
 */

import { routePath, type Route } from "./routes.js";

/**
 * How many places the stack remembers.
 *
 * A session is bounded by its own patience, and an unbounded stack is a leak that only a very long
 * session would show: the oldest place is dropped so Back stops at the hundredth visit rather than
 * growing forever. The bound is generous enough that a person never meets it on purpose.
 */
export const MAX_HISTORY = 100;

/** Where the user is, and the places they can go back to, newest last. */
export interface NavigationHistory {
  readonly past: readonly Route[];
  readonly current: Route;
}

/** A history that begins at one place, with nowhere behind it. */
export function startHistory(route: Route): NavigationHistory {
  return { past: [], current: route };
}

/** Whether there is anywhere to go back to. */
export function canGoBack(history: NavigationHistory): boolean {
  return history.past.length > 0;
}

/**
 * Whether two routes name the same place.
 *
 * Compared through {@link routePath}, so the two spellings of one place — a bare `/browse` and
 * `/browse` with an explicitly absent kind — cannot look like a move. The writer in
 * {@link ./routes.js} is the single authority on what a route's path is, which is the same property
 * that lets a sidebar route and a link agree.
 */
export function sameRoute(one: Route, other: Route): boolean {
  return routePath(one) === routePath(other);
}

/** Moves to `route`, remembering where the user was unless that is where they already are. */
export function navigate(history: NavigationHistory, route: Route): NavigationHistory {
  if (sameRoute(history.current, route)) {
    return history;
  }
  const past = [...history.past, history.current];
  return {
    past: past.length > MAX_HISTORY ? past.slice(past.length - MAX_HISTORY) : past,
    current: route,
  };
}

/** Returns to the previous place, or stands still when there is none. */
export function back(history: NavigationHistory): NavigationHistory {
  const previous = history.past[history.past.length - 1];
  if (previous === undefined) {
    return history;
  }
  return { past: history.past.slice(0, -1), current: previous };
}
