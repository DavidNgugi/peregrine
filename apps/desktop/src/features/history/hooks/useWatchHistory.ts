import { useEffect, useState } from "react";

import { normalizeIpcError, viewingHistory } from "@peregrine/ipc-client";
import type { IpcError, WatchHistoryDto } from "@peregrine/ipc-client";

import { logUiError } from "../../../lib/error-log.js";

/** The account's viewing history, newest first. */
export interface WatchHistoryView {
  readonly rows: readonly WatchHistoryDto[];
  readonly loading: boolean;
  readonly failed: IpcError | null;
}

export function useWatchHistory(): WatchHistoryView {
  const [state, setState] = useState<WatchHistoryView>({ rows: [], loading: true, failed: null });
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const rows = await viewingHistory(200);
        if (!cancelled) setState({ rows, loading: false, failed: null });
      } catch (caught: unknown) {
        const error = normalizeIpcError(caught);
        logUiError("viewingHistory", error);
        if (!cancelled) setState({ rows: [], loading: false, failed: error });
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);
  return state;
}
