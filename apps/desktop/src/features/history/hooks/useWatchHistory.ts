import { useEffect, useState } from "react";

import { normalizeIpcError, viewingHistory } from "@peregrine/ipc-client";
import type { IpcError, WatchHistoryDto } from "@peregrine/ipc-client";

import { logUiError } from "../../../lib/error-log.js";

/** The account's viewing history, newest first. */
export interface WatchHistoryView {
  readonly rows: readonly WatchHistoryDto[];
  readonly loading: boolean;
  readonly failed: IpcError | null;
  /** Re-reads after a clear, so the list never shows a row that is already gone. */
  readonly refresh: () => Promise<void>;
}

export function useWatchHistory(): WatchHistoryView {
  const [state, setState] = useState<Omit<WatchHistoryView, "refresh">>({
    rows: [],
    loading: true,
    failed: null,
  });
  const read = async (): Promise<void> => {
    try {
      const rows = await viewingHistory(200);
      setState({ rows, loading: false, failed: null });
    } catch (caught: unknown) {
      const error = normalizeIpcError(caught);
      logUiError("viewingHistory", error);
      setState({ rows: [], loading: false, failed: error });
    }
  };
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
  return { ...state, refresh: read };
}
