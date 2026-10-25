/**
 * A filter or search field that survives the screen being unmounted.
 *
 * The browser client has no persistent screen store: Back remounts a list, and a local `useState`
 * would silently reset the filter that produced the list. Session storage is the smallest store that
 * is per-window, per-profile-session, and gone when the session ends. The scroll/focus memory is
 * separate (`route-state.ts`); this remembers the query that made the screen worth returning to.
 */

import { useEffect, useState } from "react";

const PREFIX = "peregrine.view.";

export function readViewState<T>(key: string, fallback: T): T {
  try {
    if (typeof window === "undefined") return fallback;
    const raw = window.sessionStorage.getItem(PREFIX + key);
    return raw === null ? fallback : (JSON.parse(raw) as T);
  } catch {
    return fallback;
  }
}

export function writeViewState<T>(key: string, value: T): void {
  try {
    window.sessionStorage.setItem(PREFIX + key, JSON.stringify(value));
  } catch {
    // A full or disabled store only loses the next visit's convenience.
  }
}

/** The value, plus a setter that mirrors it into session storage. */
export function useViewState<T>(key: string, initial: T): [T, (value: T) => void] {
  const [value, setValue] = useState<T>(() => readViewState(key, initial));
  useEffect(() => {
    writeViewState(key, value);
  }, [key, value]);
  return [value, setValue];
}
