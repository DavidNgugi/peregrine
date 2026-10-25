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

import {
  eventTopics,
  libraryStorage,
  listLibraryTitles,
  normalizeIpcError,
} from "@peregrine/ipc-client";
import type { LibraryStorageDto, LibraryTitleDto } from "@peregrine/ipc-client";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useCallback, useEffect, useState } from "react";
import { logUiError } from "../../../lib/error-log.js";
import { eventPayload, jobPhase } from "../../../lib/job-events.js";
import { byRecentlyAdded } from "./library-order.js";

/** What the library holds, and what the screen needs to know about the read. */
export interface LibraryTitles {
  /** The titles, newest first; empty until the read answers. */
  readonly titles: readonly LibraryTitleDto[];
  /** The visible library's stored media bytes and file count, or null until it answers. */
  readonly storage: LibraryStorageDto | null;
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
 * Whether a `job://state` envelope is a provider fill finishing successfully.
 *
 * Exported because the listener cannot be rendered in this repository's Node test environment:
 * which events count as a completed fill is the part that can be wrong, and it is a pure question.
 */
export function providerFillFinished(envelope: unknown): boolean {
  const payload = eventPayload(envelope);
  return (
    payload !== null &&
    payload["kind"] === "provider.refresh_title" &&
    jobPhase(payload["phase"]) === "succeeded"
  );
}

/**
 * The titles the library holds.
 *
 * Read on mount, and again whenever a provider fill finishes. A scan writes a stub from a filename,
 * so the read that follows a scan has no poster, synopsis, or score for it — those are the fill's,
 * and the fill is a job whose success is the core saying the row is complete. Watching that one
 * signal rather than polling is what makes a matched title appear in the grid without a visit to its
 * own screen, which is the only surface that re-reads for itself (`PG-E14-17`).
 */
export function useLibraryTitles(): LibraryTitles {
  const [state, setState] = useState<Omit<LibraryTitles, "refresh">>({
    titles: [],
    storage: null,
    loading: true,
    failed: false,
  });

  const read = useCallback(async (): Promise<void> => {
    try {
      const [titles, storage] = await Promise.all([
        listLibraryTitles(),
        // The total is an addition to the read, not a precondition: a listener that cannot answer
        // it still draws the library rather than turning the whole screen into an error.
        libraryStorage().catch((caught: unknown) => {
          logUiError("libraryStorage", normalizeIpcError(caught));
          return null;
        }),
      ]);
      setState({ titles: byRecentlyAdded(titles), storage, loading: false, failed: false });
    } catch (caught: unknown) {
      logUiError("listLibraryTitles", normalizeIpcError(caught));
      setState({ titles: [], storage: null, loading: false, failed: true });
    }
  }, []);

  useEffect(() => {
    void read();
  }, [read]);

  // The fill writes what the read above could not have seen, so its success is the one moment a
  // re-read is both useful and final. A fill that failed changes nothing a tile draws, and one that
  // matched nothing is a success the core reports — the read it costs is local and small.
  useEffect(() => {
    let cancelled = false;
    let unlisten: UnlistenFn | null = null;

    void listen<unknown>(eventTopics.jobState.name, (event) => {
      if (providerFillFinished(event.payload)) {
        void read();
      }
    }).then((release) => {
      if (cancelled) {
        release();
      } else {
        unlisten = release;
      }
    });

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [read]);

  return { ...state, refresh: read };
}
