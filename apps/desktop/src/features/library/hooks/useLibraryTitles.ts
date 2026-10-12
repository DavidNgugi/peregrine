/**
 * The hook layer for the library: what the scans have actually put in it.
 *
 * Three states, kept apart for the reason the other reads give: still reading, failed, and
 * genuinely empty. The middle one matters more here than anywhere else — a library whose read
 * failed and a library with nothing in it look identical on screen, and the second is what a user
 * sees the first time they scan a folder that turned out to hold nothing Peregrine recognises.
 *
 * A title the core sends is a *stub* until a provider sync fills it in, which the hook passes
 * through rather than hiding: the screen can then say what it has without implying it has more.
 */

import { listLibraryTitles, normalizeIpcError } from "@peregrine/ipc-client";
import type { LibraryTitleDto } from "@peregrine/ipc-client";
import { useCallback, useEffect, useState } from "react";
import { logUiError } from "../../../lib/error-log.js";

/** What the library holds, and what the screen needs to know about the read. */
export interface LibraryTitles {
  /** The titles, newest first; empty until the read answers. */
  readonly titles: readonly LibraryTitleDto[];
  /** The read is still in flight. */
  readonly loading: boolean;
  /** The read failed, so the list is unknown rather than empty. */
  readonly failed: boolean;
  /**
   * Re-reads the library.
   *
   * What the screen calls when a scan it started reaches a terminal phase: a scan is the only thing
   * that changes this list, so watching one finish is the whole of the subscription this screen
   * needs — polling would ask the same question at a cost nobody sees.
   */
  readonly refresh: () => Promise<void>;
}

/**
 * The titles the library holds.
 *
 * Read once on mount. A scan is what changes this list, and a scan is a job the shell already
 * watches — so the next step here is to re-read when one finishes rather than to poll, which is a
 * subscription the jobs bridge owns (`PG-E14-04`).
 */
export function useLibraryTitles(): LibraryTitles {
  const [state, setState] = useState<Omit<LibraryTitles, "refresh">>({
    titles: [],
    loading: true,
    failed: false,
  });

  const read = useCallback(async (): Promise<void> => {
    try {
      const titles = await listLibraryTitles();
      setState({ titles, loading: false, failed: false });
    } catch (caught: unknown) {
      logUiError("listLibraryTitles", normalizeIpcError(caught));
      setState({ titles: [], loading: false, failed: true });
    }
  }, []);

  useEffect(() => {
    void read();
  }, [read]);

  return { ...state, refresh: read };
}
