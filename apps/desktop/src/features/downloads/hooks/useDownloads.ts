/**
 * The Downloads read: the transfers the acquisition service has admitted.
 *
 * **Event-driven, with a poll kept only as a safety net.** The bus already coalesces the engine's
 * statistics to its tick, so `download://stats` is the signal that a row moved and `job://state`
 * is the signal that one exists at all — a job event is what creates the first row, so listening
 * only to statistics would miss a queue going from empty to one. The fallback timer exists for the
 * case the bus cannot deliver (a refused sink is counted as degraded, not retried), and it is slow
 * because it is no longer the path a person waits on.
 */

import { eventTopics, listDownloads, normalizeIpcError } from "@peregrine/ipc-client";
import type { DownloadJobDto } from "@peregrine/ipc-client";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";
import { logUiError } from "../../../lib/error-log.js";
import { eventPayload } from "../../../lib/job-events.js";

/** How often the queue is re-read when no event has arrived.
 *
 * A safety net rather than the primary path: the `download://stats` and `job://state` topics carry
 * the changes, and this only covers a bus that could not deliver. */
const FALLBACK_POLL_INTERVAL_MS = 15_000;

/** The shortest gap between two event-driven reads.
 *
 * The bus delivers at most one update per topic per tick, so this is not about a producer's rate;
 * it is about two topics landing together (a state change and a statistics update for the same
 * row) becoming one request rather than two. */
const THROTTLE_MS = 500;

/** What the Downloads screen draws, and what it needs to know about the read. */
export interface Downloads {
  /** The queue, newest first as the core ordered it. Empty until the read answers. */
  readonly downloads: readonly DownloadJobDto[];
  /** The read is in flight. */
  readonly loading: boolean;
  /** The read failed, so the queue is unknown rather than empty. */
  readonly failed: boolean;
  /** Re-reads the queue, which is what every move invalidates. */
  readonly refresh: () => Promise<void>;
}

/** The download queue, re-read when the core says something moved. */
export function useDownloads(): Downloads {
  const [state, setState] = useState<Omit<Downloads, "refresh">>({
    downloads: [],
    loading: true,
    failed: false,
  });

  const read = useCallback(async (): Promise<void> => {
    try {
      const downloads = await listDownloads();
      setState({ downloads, loading: false, failed: false });
    } catch (caught: unknown) {
      logUiError("listDownloads", normalizeIpcError(caught));
      setState({ downloads: [], loading: false, failed: true });
    }
  }, []);

  const lastRead = useRef(0);
  const pending = useRef<ReturnType<typeof setTimeout> | null>(null);

  // An event is a trigger to re-read, never a source of truth: the payload is validated for shape
  // and then discarded, because the row it describes is the repository's answer, not the event's.
  const schedule = useCallback((): void => {
    const elapsed = Date.now() - lastRead.current;
    if (elapsed >= THROTTLE_MS) {
      lastRead.current = Date.now();
      void read();
      return;
    }
    if (pending.current !== null) {
      return;
    }
    pending.current = setTimeout(() => {
      pending.current = null;
      lastRead.current = Date.now();
      void read();
    }, THROTTLE_MS - elapsed);
  }, [read]);

  useEffect(() => {
    void read();
  }, [read]);

  useEffect(() => {
    const unlisteners: UnlistenFn[] = [];
    let cancelled = false;

    const subscribe = (topic: string): void => {
      void listen<unknown>(topic, (event) => {
        if (eventPayload(event.payload) === null) {
          return;
        }
        schedule();
      }).then((unlisten) => {
        if (cancelled) {
          unlisten();
        } else {
          unlisteners.push(unlisten);
        }
      });
    };

    subscribe(eventTopics.downloadStats.name);
    subscribe(eventTopics.jobState.name);

    return () => {
      cancelled = true;
      for (const unlisten of unlisteners) {
        unlisten();
      }
      if (pending.current !== null) {
        clearTimeout(pending.current);
        pending.current = null;
      }
    };
  }, [schedule]);

  useEffect(() => {
    const timer = setInterval(() => {
      void read();
    }, FALLBACK_POLL_INTERVAL_MS);
    return () => {
      clearInterval(timer);
    };
  }, [read]);

  return { ...state, refresh: read };
}
