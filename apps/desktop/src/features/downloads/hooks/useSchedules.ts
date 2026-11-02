import { useCallback, useEffect, useState } from "react";

import {
  cancelDownloadSchedule,
  downloadSchedules,
  normalizeIpcError,
  scheduleDownload,
} from "@peregrine/ipc-client";
import type { IpcError, ScheduledDownloadDto } from "@peregrine/ipc-client";

import { logUiError } from "../../../lib/error-log.js";

/** How often the list is re-read while the screen is open.
 *
 * The ticker is the only writer, and it can fire at any moment, so a list read once on mount
 * would keep showing a request the ticker has already answered. The read is one indexed query
 * against SQLite and touches no provider. */
const REFRESH_INTERVAL_MS = 30_000;

/** The schedules this account holds, and the two moves the section offers. */
export interface SchedulesView {
  readonly rows: readonly ScheduledDownloadDto[];
  readonly failed: IpcError | null;
  readonly cancel: (id: number) => void;
  /** Puts a failed request back in the waiting list, unchanged, for the ticker to try again. */
  readonly retry: (row: ScheduledDownloadDto) => void;
}

export function useSchedules(): SchedulesView {
  const [rows, setRows] = useState<readonly ScheduledDownloadDto[]>([]);
  const [failed, setFailed] = useState<IpcError | null>(null);

  const load = useCallback(() => {
    void downloadSchedules()
      .then((found) => {
        setRows(found);
        setFailed(null);
      })
      .catch((caught: unknown) => {
        const error = normalizeIpcError(caught);
        logUiError("downloadSchedules", error);
        setFailed(error);
      });
  }, []);

  useEffect(() => {
    load();
    const timer = setInterval(load, REFRESH_INTERVAL_MS);
    return () => clearInterval(timer);
  }, [load]);

  const cancel = useCallback((id: number) => {
    void cancelDownloadSchedule(id)
      .then(() => {
        setRows((current) => current.filter((row) => row.id !== id));
      })
      .catch((caught: unknown) => {
        logUiError("cancelDownloadSchedule", normalizeIpcError(caught));
      });
  }, []);

  const retry = useCallback(
    (row: ScheduledDownloadDto) => {
      void scheduleDownload(row.titleId, row.seasonNumber, row.episodeNumber, row.releaseOn)
        .then(load)
        .catch((caught: unknown) => {
          logUiError("scheduleDownload", normalizeIpcError(caught));
        });
    },
    [load],
  );

  return { rows, failed, cancel, retry };
}
