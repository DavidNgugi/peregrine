/**
 * Pause, resume, and remove, with the queue re-read after each.
 *
 * One in flight at a time, named by `busyId`, so a row's controls disable while its own move is
 * running and a second click cannot race the first. The failure is kept rather than thrown: a screen
 * that unmounted on a refused move would take the queue with it.
 */

import {
  clearFinishedDownloads,
  deleteDownload,
  dismissDownload,
  normalizeIpcError,
  pauseDownload,
  planDownloadDeletion,
  removeDownload,
  resumeDownload,
} from "@peregrine/ipc-client";
import type { DeletionPlanDto, IpcError } from "@peregrine/ipc-client";
import { useCallback, useState } from "react";
import { logUiError } from "../../../lib/error-log.js";

/** The moves a row offers. */
export interface DownloadActions {
  /** The row whose move is in flight, or `null`. */
  readonly busyId: number | null;
  /** The bulk clear is in flight. */
  readonly clearing: boolean;
  /** The row whose deletion plan is being read, or `null`. */
  readonly planningId: number | null;
  /** What the last move failed with, or `null`. */
  readonly error: IpcError | null;
  readonly pause: (id: number) => Promise<void>;
  readonly resume: (id: number) => Promise<void>;
  readonly remove: (id: number) => Promise<void>;
  /** Clears one finished entry from the queue. */
  readonly dismiss: (id: number) => Promise<void>;
  /** Clears every finished entry from the queue. */
  readonly clearFinished: () => Promise<void>;
  /** Reads what deleting one row would remove, or `null` when the read failed. */
  readonly planDeletion: (id: number) => Promise<DeletionPlanDto | null>;
  /** Moves one row's files to the trash and clears the entry. */
  readonly deleteFiles: (id: number) => Promise<void>;
}

/** The moves, each re-reading the queue through `onChanged`. */
export function useDownloadActions(onChanged: () => Promise<void>): DownloadActions {
  const [state, setState] = useState<{
    busyId: number | null;
    clearing: boolean;
    planningId: number | null;
    error: IpcError | null;
  }>({ busyId: null, clearing: false, planningId: null, error: null });

  const failed = useCallback((label: string, caught: unknown): void => {
    const error = normalizeIpcError(caught);
    logUiError(label, error);
    setState({ busyId: null, clearing: false, planningId: null, error });
  }, []);

  const move = useCallback(
    async (
      id: number,
      call: (id: number) => Promise<unknown>,
      label: string,
    ): Promise<void> => {
      setState({ busyId: id, clearing: false, planningId: null, error: null });
      try {
        await call(id);
        await onChanged();
        setState({ busyId: null, clearing: false, planningId: null, error: null });
      } catch (caught: unknown) {
        failed(label, caught);
      }
    },
    [failed, onChanged],
  );

  const pause = useCallback((id: number) => move(id, pauseDownload, "pauseDownload"), [move]);
  const resume = useCallback((id: number) => move(id, resumeDownload, "resumeDownload"), [move]);
  const remove = useCallback((id: number) => move(id, removeDownload, "removeDownload"), [move]);
  const dismiss = useCallback((id: number) => move(id, dismissDownload, "dismissDownload"), [move]);
  const deleteFiles = useCallback(
    (id: number) => move(id, deleteDownload, "deleteDownload"),
    [move],
  );

  // The plan is a read rather than a move: it changes nothing and is not followed by a re-read of
  // the queue. It is still one-in-flight, because the dialog it feeds must not race a second click.
  const planDeletion = useCallback(
    async (id: number): Promise<DeletionPlanDto | null> => {
      setState({ busyId: null, clearing: false, planningId: id, error: null });
      try {
        const plan = await planDownloadDeletion(id);
        setState({ busyId: null, clearing: false, planningId: null, error: null });
        return plan;
      } catch (caught: unknown) {
        failed("planDownloadDeletion", caught);
        return null;
      }
    },
    [failed],
  );

  const clearFinished = useCallback(async (): Promise<void> => {
    setState({ busyId: null, clearing: true, planningId: null, error: null });
    try {
      await clearFinishedDownloads();
      await onChanged();
      setState({ busyId: null, clearing: false, planningId: null, error: null });
    } catch (caught: unknown) {
      failed("clearFinishedDownloads", caught);
    }
  }, [failed, onChanged]);

  return { ...state, pause, resume, remove, dismiss, clearFinished, planDeletion, deleteFiles };
}
