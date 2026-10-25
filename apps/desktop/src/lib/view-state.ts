/**
 * A filter or search field that survives a screen unmounting.
 *
 * The desktop shell swaps one screen for another rather than keeping every screen mounted, so local
 * `useState` resets the filters that produced the list the user is returning to. Session storage is
 * the smallest store that is per-window, per-session, and does not need a server round trip.
 */

import { useEffect, useState } from "react";

const PREFIX = "peregrine.view.";

/** Reads one stored value, or the fallback when storage is disabled or holds junk. */
export function readViewState<T>(key: string | undefined, fallback: T): T {
  try {
    if (key === undefined || typeof window === "undefined") return fallback;
    const raw = window.sessionStorage.getItem(PREFIX + key);
    return raw === null ? fallback : (JSON.parse(raw) as T);
  } catch {
    return fallback;
  }
}

/** Mirrors one value into session storage; a failed write only loses the next visit's convenience. */
export function writeViewState<T>(key: string | undefined, value: T): void {
  try {
    if (key === undefined) return;
    window.sessionStorage.setItem(PREFIX + key, JSON.stringify(value));
  } catch {
    // A full or disabled store is not an error the screen can act on.
  }
}

/** The value, plus a setter that mirrors it into session storage. */
export function useViewState<T>(key: string | undefined, initial: T): [T, (value: T) => void] {
  const [value, setValue] = useState<T>(() => readViewState(key, initial));
  useEffect(() => {
    writeViewState(key, value);
  }, [key, value]);
  return [value, setValue];
}
