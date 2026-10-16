/**
 * Pause, resume, and remove, with the queue re-read after each.
 *
 * One in flight at a time, named by `busyId`, so a row's controls disable while its own move is
 * running and a second click cannot race the first. The failure is kept rather than thrown: a screen
 * that unmounted on a refused move would take the queue with it.
 */

import {
  normalizeIpcError,
  pauseDownload,
  removeDownload,
  resumeDownload,
} from "@peregrine/ipc-client";
import type { IpcError } from "@peregrine/ipc-client";
import { useCallback, useState } from "react";
import { logUiError } from "../../../lib/error-log.js";

/** The moves a row offers. */
export interface DownloadActions {
  /** The row whose move is in flight, or `null`. */
  readonly busyId: number | null;
  /** What the last move failed with, or `null`. */
  readonly error: IpcError | null;
  readonly pause: (id: number) => Promise<void>;
  readonly resume: (id: number) => Promise<void>;
  readonly remove: (id: number) => Promise<void>;
}

/** The moves, each re-reading the queue through `onChanged`. */
export function useDownloadActions(onChanged: () => Promise<void>): DownloadActions {
  const [state, setState] = useState<{ busyId: number | null; error: IpcError | null }>({
    busyId: null,
    error: null,
  });

  const move = useCallback(
    async (
      id: number,
      call: (id: number) => Promise<unknown>,
      label: string,
    ): Promise<void> => {
      setState({ busyId: id, error: null });
      try {
        await call(id);
        await onChanged();
        setState({ busyId: null, error: null });
      } catch (caught: unknown) {
        const error = normalizeIpcError(caught);
        logUiError(label, error);
        setState({ busyId: null, error });
      }
    },
    [onChanged],
  );

  const pause = useCallback((id: number) => move(id, pauseDownload, "pauseDownload"), [move]);
  const resume = useCallback((id: number) => move(id, resumeDownload, "resumeDownload"), [move]);
  const remove = useCallback((id: number) => move(id, removeDownload, "removeDownload"), [move]);

  return { ...state, pause, resume, remove };
}
