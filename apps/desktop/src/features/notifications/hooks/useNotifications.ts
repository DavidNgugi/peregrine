/**
 * The in-app notification centre.
 *
 * R31 makes the centre row the record and the OS toast the bonus, so this read is how a
 * notification reaches the user at all while the notifier adapter is unwritten. It holds rendered
 * copy only: the core scanned the row for a magnet, path, or secret before it was stored.
 */

import {
  dismissNotification,
  eventTopics,
  listNotifications,
  markAllNotificationsRead,
  markNotificationRead,
  normalizeIpcError,
  type NotificationDto,
} from "@peregrine/ipc-client";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useCallback, useEffect, useState } from "react";
import { logUiError } from "../../../lib/error-log.js";
import { eventPayload } from "../../../lib/job-events.js";

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
  /** Marks one row read. */
  readonly markRead: (id: number) => Promise<void>;
  /** Marks every unread row read. */
  readonly markAllRead: () => Promise<void>;
  /** Dismisses one row, so it leaves the list. */
  readonly dismiss: (id: number) => Promise<void>;
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

  // A recorded row is a trigger to re-read, never a row to render: the payload carries no copy, so
  // the bell can only show what the centre answers with. Without this the count waited for the next
  // open, which is the behaviour this replaces.
  useEffect(() => {
    const unlisteners: UnlistenFn[] = [];
    let cancelled = false;

    void listen<unknown>(eventTopics.notificationRaised.name, (event) => {
      if (eventPayload(event.payload) === null) {
        return;
      }
      void refresh();
    }).then((unlisten) => {
      if (cancelled) {
        unlisten();
      } else {
        unlisteners.push(unlisten);
      }
    });

    return () => {
      cancelled = true;
      for (const unlisten of unlisteners) {
        unlisten();
      }
    };
  }, [refresh]);

  // Every mutation re-reads rather than patching local state: whether a row is read is the
  // repository's answer, and a second source of truth would drift from it.
  const markRead = useCallback(
    async (id: number): Promise<void> => {
      try {
        await markNotificationRead(id);
        await refresh();
      } catch (caught: unknown) {
        logUiError("notifications.markRead", normalizeIpcError(caught));
      }
    },
    [refresh],
  );

  const markAllRead = useCallback(async (): Promise<void> => {
    try {
      await markAllNotificationsRead();
      await refresh();
    } catch (caught: unknown) {
      logUiError("notifications.markAllRead", normalizeIpcError(caught));
    }
  }, [refresh]);

  const dismiss = useCallback(
    async (id: number): Promise<void> => {
      try {
        await dismissNotification(id);
        await refresh();
      } catch (caught: unknown) {
        logUiError("notifications.dismiss", normalizeIpcError(caught));
      }
    },
    [refresh],
  );

  return { rows, loading, failed, refresh, markRead, markAllRead, dismiss };
}
