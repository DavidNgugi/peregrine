/**
 * The title page's acquisition control: ask the core to find and queue a release.
 *
 * `acquire` starts a job and answers its id, because the work leaves the machine — search, metadata,
 * the gate — and a command that waited would be a frozen page (R10). The screen therefore reports
 * "started" rather than "downloading"; the queue is the Downloads screen's to draw.
 */

import { acquire, normalizeIpcError } from "@peregrine/ipc-client";
import type { IpcError } from "@peregrine/ipc-client";
import { useCallback, useState } from "react";
import { logUiError } from "../../../lib/error-log.js";

/** What the title page's Download control reads. */
export interface AcquireControl {
  /** A start is in flight. */
  readonly starting: boolean;
  /** A start was accepted, so the release is being found and queued. */
  readonly started: boolean;
  /** What the last start refused with, or `null`. */
  readonly error: IpcError | null;
  /** Asks the core to acquire the title. */
  readonly start: (titleId: number) => Promise<void>;
}

/** The acquisition control for one title. */
export function useAcquire(): AcquireControl {
  const [state, setState] = useState<Omit<AcquireControl, "start">>({
    starting: false,
    started: false,
    error: null,
  });

  const start = useCallback(async (titleId: number): Promise<void> => {
    setState({ starting: true, started: false, error: null });
    try {
      await acquire(titleId);
      setState({ starting: false, started: true, error: null });
    } catch (caught: unknown) {
      const error = normalizeIpcError(caught);
      logUiError("acquire", error);
      setState({ starting: false, started: false, error });
    }
  }, []);

  return { ...state, start };
}
