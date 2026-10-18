import { normalizeIpcError, watchAvailability } from "@peregrine/ipc-client";
import type { IpcError, WatchAvailabilityDto } from "@peregrine/ipc-client";
import { useEffect, useMemo, useRef, useState } from "react";

import { logUiError } from "../../../lib/error-log.js";
import { currentRegion } from "../../../lib/region.js";

/** Where a title can be watched, or the reason the read did not answer. */
export interface WatchAvailabilityState {
  /** The answer, or `null` when it has not been read yet or the read failed. */
  readonly data: WatchAvailabilityDto | null;
  /** The read failed. The section hides rather than claiming there is nothing to watch. */
  readonly failed: IpcError | null;
}

/**
 * Reads a title's cached availability for the region this machine is in.
 *
 * **The read is cache-only and the region is the locale's.** Fetching is the title refresh job's,
 * which the screen already starts when the core says `refreshDue`; `refreshes` is that job's
 * stop count, so a refresh that wrote availability is drawn without leaving the page.
 */
export function useWatchAvailability(
  titleId: number | null,
  refreshes: number,
): WatchAvailabilityState {
  const region = useMemo(currentRegion, []);
  const [state, setState] = useState<WatchAvailabilityState>({ data: null, failed: null });
  // A read that lands after the route moved on is about a title nobody is looking at.
  const version = useRef(0);

  useEffect(() => {
    if (titleId === null) {
      version.current += 1;
      setState({ data: null, failed: null });
      return;
    }
    const mine = ++version.current;
    void watchAvailability(titleId, region)
      .then((data) => {
        if (mine === version.current) {
          setState({ data, failed: null });
        }
      })
      .catch((caught: unknown) => {
        if (mine !== version.current) {
          return;
        }
        const error = normalizeIpcError(caught);
        logUiError("watchAvailability", error);
        setState({ data: null, failed: error });
      });
  }, [titleId, region, refreshes]);

  return state;
}
