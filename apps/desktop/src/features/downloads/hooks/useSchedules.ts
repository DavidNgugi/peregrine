import { useCallback, useEffect, useState } from "react";

import {
  cancelDownloadSchedule,
  downloadSchedules,
  normalizeIpcError,
} from "@peregrine/ipc-client";
import type { IpcError, ScheduledDownloadDto } from "@peregrine/ipc-client";

import { logUiError } from "../../../lib/error-log.js";

/** The schedules this account holds, and the one move the section offers. */
export interface SchedulesView {
  readonly rows: readonly ScheduledDownloadDto[];
  readonly failed: IpcError | null;
  readonly cancel: (id: number) => void;
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

  return { rows, failed, cancel };
}
