import { useCallback, useEffect, useState } from "react";

import {
  addToWatchlist,
  normalizeIpcError,
  removeFromWatchlist,
  watchlist as readWatchlist,
} from "@peregrine/ipc-client";
import type { IpcError, WatchlistEntryDto } from "@peregrine/ipc-client";

import { logUiError } from "../../../lib/error-log.js";

/** The account's saved titles, and the one move the detail page and the rail share. */
export interface WatchlistState {
  readonly rows: readonly WatchlistEntryDto[];
  readonly failed: IpcError | null;
  readonly has: (titleId: number) => boolean;
  readonly toggle: (titleId: number) => void;
}

export function useWatchlist(): WatchlistState {
  const [rows, setRows] = useState<readonly WatchlistEntryDto[]>([]);
  const [failed, setFailed] = useState<IpcError | null>(null);

  const load = useCallback(() => {
    void readWatchlist()
      .then((found) => {
        setRows(found);
        setFailed(null);
      })
      .catch((caught: unknown) => {
        const error = normalizeIpcError(caught);
        logUiError("watchlist", error);
        setFailed(error);
      });
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const has = useCallback((titleId: number) => rows.some((row) => row.titleId === titleId), [rows]);

  const toggle = useCallback(
    (titleId: number) => {
      const saved = rows.some((row) => row.titleId === titleId);
      const move = saved ? removeFromWatchlist(titleId) : addToWatchlist(titleId);
      void move
        .then(() => {
          load();
        })
        .catch((caught: unknown) => {
          const error = normalizeIpcError(caught);
          logUiError("watchlist.toggle", error);
          setFailed(error);
        });
    },
    [load, rows],
  );

  return { rows, failed, has, toggle };
}
