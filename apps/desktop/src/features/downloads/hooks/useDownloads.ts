/**
 * The Downloads read: the transfers the acquisition service has admitted.
 *
 * **Polled, and that is a cost taken knowingly.** What moves a row is the engine's statistics tick,
 * which is not a job event — a screen that only heard the lifecycle would draw a bar that never
 * fills. The timer exists only while some row can still change, so an idle screen asks nothing.
 */

import { listDownloads, normalizeIpcError } from "@peregrine/ipc-client";
import type { DownloadJobDto } from "@peregrine/ipc-client";
import { useCallback, useEffect, useState } from "react";
import { logUiError } from "../../../lib/error-log.js";

/** How often a row is re-read while any of them can still move. */
const POLL_INTERVAL_MS = 2_000;

/** A state from which no further change comes, so polling can stop. */
const TERMINAL_STATES: ReadonlySet<string> = new Set([
  "completed",
  "failed",
  "blocked",
  "removed",
]);

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

/** The download queue, re-read while it can change. */
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

  useEffect(() => {
    void read();
  }, [read]);

  const active = state.downloads.some((download) => !TERMINAL_STATES.has(download.state));
  useEffect(() => {
    if (!active) {
      return;
    }
    const timer = setInterval(() => {
      void read();
    }, POLL_INTERVAL_MS);
    return () => {
      clearInterval(timer);
    };
  }, [active, read]);

  return { ...state, refresh: read };
}
