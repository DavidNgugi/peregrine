/**
 * The hook layer for the files the scans could not place.
 *
 * A queue rather than a list: every row is a question waiting for an answer, and the count is the
 * one number on this screen that is *work* rather than state. The read is scoped like every
 * user-facing read, so a file belonging to a title the account may not see is not offered to it.
 *
 * Three states again, and the middle one is the one that matters: a queue that failed to load and an
 * empty queue look identical on screen, and "nothing needs your attention" is exactly the wrong
 * thing to say when the read did not answer.
 */

import { listUnmatchedFiles, matchUnmatchedFile, normalizeIpcError } from "@peregrine/ipc-client";
import type { IpcError, UnmatchedFileDto } from "@peregrine/ipc-client";
import { useCallback, useEffect, useState } from "react";
import { logUiError } from "../../../lib/error-log.js";
import { messageOf } from "../../../lib/ipc-message.js";

/** The parked files, and what the screen needs to know about the read. */
export interface UnmatchedQueue {
  /** The files waiting for an answer, newest first. */
  readonly files: readonly UnmatchedFileDto[];
  /** The read is still in flight. */
  readonly loading: boolean;
  /** The read failed, so the queue is unknown rather than empty. */
  readonly failed: boolean;
  /** An answer is in flight, so the buttons that answer are disabled. */
  readonly busy: boolean;
  /** What the last answer failed with, in the core's own words. */
  readonly error: string | null;
  /**
   * Answers one parked file.
   *
   * The suggestion is offered when the core sent one, and it is the better answer: it joins a title
   * the library already holds rather than creating a near-duplicate of it. The parse's reading is the
   * fallback, and a row with no suggestion is still answerable.
   */
  readonly match: (file: UnmatchedFileDto) => Promise<void>;
  /** Re-reads the queue. */
  readonly refresh: () => Promise<void>;
}

/** The parked files, and the action that answers one. */
export function useUnmatchedFiles(): UnmatchedQueue {
  const [state, setState] = useState<{
    files: readonly UnmatchedFileDto[];
    loading: boolean;
    failed: boolean;
    busy: boolean;
    error: string | null;
  }>({ files: [], loading: true, failed: false, busy: false, error: null });

  const refresh = useCallback(async (): Promise<void> => {
    try {
      const files = await listUnmatchedFiles();
      setState((previous) => ({ ...previous, files, loading: false, failed: false }));
    } catch (caught: unknown) {
      const error: IpcError = normalizeIpcError(caught);
      logUiError("listUnmatchedFiles", error);
      setState((previous) => ({ ...previous, files: [], loading: false, failed: true }));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const match = useCallback(
    async (file: UnmatchedFileDto): Promise<void> => {
      setState((previous) => ({ ...previous, busy: true, error: null }));
      try {
        // The choice is made here rather than by the screen: which title an answer means is a fact
        // about the row, and a caller that had to remember to pass it would eventually forget.
        await matchUnmatchedFile(
          file.id,
          file.suggestedTitleName === null ? "parsed" : "suggested",
        );
        await refresh();
      } catch (caught: unknown) {
        const error = normalizeIpcError(caught);
        logUiError("matchUnmatchedFile", error);
        setState((previous) => ({ ...previous, error: messageOf(error) }));
      } finally {
        setState((previous) => ({ ...previous, busy: false }));
      }
    },
    [refresh],
  );

  return { ...state, match, refresh };
}
