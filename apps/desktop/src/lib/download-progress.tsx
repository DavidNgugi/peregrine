import { createContext, useContext, type ReactElement, type ReactNode } from "react";

import type { DownloadProgress } from "./download-status.js";

const EMPTY: DownloadProgress = new Map();

const DownloadProgressContext = createContext<DownloadProgress>(EMPTY);

/**
 * How far each title's transfer has got, for every card under the shell.
 *
 * A context rather than a prop: the map is the same on every screen, and threading it through four
 * screen wrappers would add a parameter to each of them for a value none of them reasons about.
 * Screens rendered outside the provider — the tests — read an empty map, which draws no overlay.
 */
export function DownloadProgressProvider({
  children,
  value,
}: {
  readonly children: ReactNode;
  readonly value: DownloadProgress;
}): ReactElement {
  return (
    <DownloadProgressContext.Provider value={value}>{children}</DownloadProgressContext.Provider>
  );
}

export function useDownloadProgress(): DownloadProgress {
  return useContext(DownloadProgressContext);
}
