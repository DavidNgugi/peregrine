/**
 * The Continue Watching rail's data layer: what the core says the account is part-way through.
 *
 * **One read per visit, and no polling.** The only writer is the player, which is another screen of
 * this same page, so coming back to Discover is what makes the rail current — a poll would spend IPC
 * to learn what a remount already knows, and the core's own order is "most recently updated first".
 */

import { continueWatching, normalizeIpcError } from "@peregrine/ipc-client";
import type { ContinueWatchingDto } from "@peregrine/ipc-client";
import { useCallback, useEffect, useState } from "react";

import { logUiError } from "../../../lib/error-log.js";

/** What the rail draws, and whether the read is still in the air. */
export interface ContinueWatchingView {
  readonly rows: readonly ContinueWatchingDto[];
  readonly loading: boolean;
  readonly failed: boolean;
}

/** The account's in-progress titles, most recently watched first. */
export function useContinueWatching(): ContinueWatchingView {
  const [state, setState] = useState<{
    rows: readonly ContinueWatchingDto[];
    loading: boolean;
    failed: boolean;
  }>({ rows: [], loading: true, failed: false });

  const read = useCallback(async (): Promise<void> => {
    try {
      const rows = await continueWatching();
      setState({ rows, loading: false, failed: false });
    } catch (caught: unknown) {
      logUiError("continueWatching", normalizeIpcError(caught));
      setState((previous) => ({ ...previous, loading: false, failed: true }));
    }
  }, []);

  useEffect(() => {
    void read();
  }, [read]);

  return state;
}
