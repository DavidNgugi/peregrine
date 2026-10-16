/**
 * The in-app notification centre.
 *
 * R31 makes the centre row the record and the OS toast the bonus, so this read is how a
 * notification reaches the user at all while the notifier adapter is unwritten. It holds rendered
 * copy only: the core scanned the row for a magnet, path, or secret before it was stored.
 */

import { listNotifications, normalizeIpcError, type NotificationDto } from "@peregrine/ipc-client";
import { useCallback, useEffect, useState } from "react";
import { logUiError } from "../../../lib/error-log.js";

/** The recent centre rows and the operation that re-reads them. */
export interface Notifications {
  /** The rows, newest first. */
  readonly rows: NotificationDto[];
  /** The first read is still in flight. */
  readonly loading: boolean;
  /** The read failed, so the answer is unknown rather than "nothing happened". */
  readonly failed: boolean;
  /** Re-reads the centre. */
  readonly refresh: () => Promise<void>;
}

/** How many rows the centre panel shows. */
const LIMIT = 20;

export function useNotifications(): Notifications {
  const [rows, setRows] = useState<NotificationDto[]>([]);
  const [loading, setLoading] = useState(true);
  const [failed, setFailed] = useState(false);

  const refresh = useCallback(async (): Promise<void> => {
    try {
      setRows(await listNotifications(LIMIT));
      setFailed(false);
    } catch (caught: unknown) {
      logUiError("notifications.list", normalizeIpcError(caught));
      setFailed(true);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  return { rows, loading, failed, refresh };
}
