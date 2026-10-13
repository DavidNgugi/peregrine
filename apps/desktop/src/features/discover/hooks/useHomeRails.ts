/**
 * The data layer behind Discover: the rails the core already holds, and the job that fills them.
 *
 * **The read is cache-first and the screen paints from it** (`FLOWS F2`): `homeRails` answers from
 * the catalogue and the artwork cache and never waits for a provider, so a cold start draws every
 * rail's heading and its empty state before any request has left the machine. The fetch is a job
 * whose terminal phase triggers one re-read — no polling, no spinner (`DESIGN_SYSTEM §11` rule 2).
 *
 * **The refresh is followed by its *journal* row and not by its scheduler id.** A submission that
 * coalesced onto a fetch already running has no scheduler id of its own
 * (`peregrine_app::StartedJob::job` is nil for one), and every `job://state` payload carries both
 * ids — so the journal id is the one a screen can always match on. That is what makes two presses
 * of Refresh, or two visits to this screen, land on one fetch and one re-read.
 *
 * Every event is read through `lib/job-events.ts`, which is where the envelope rule lives, so this
 * hook and the library scan cannot disagree about which payloads are readable.
 */

import { eventTopics, homeRails, normalizeIpcError, refreshRails } from "@peregrine/ipc-client";
import type { HomeRailDto, IpcError } from "@peregrine/ipc-client";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";
import { logUiError } from "../../../lib/error-log.js";
import { followRails, NOT_FOLLOWING, type RailsFollow } from "./rails-follow.js";

/** What the Discover screen renders and what it needs to know about the reads. */
export interface HomeRailsView {
  /** The rails, in the order the core lists them. Empty until the first read answers. */
  readonly rails: readonly HomeRailDto[];
  /** A read is in flight, which is not a state the screen draws as a progress bar. */
  readonly loading: boolean;
  /** The last read failed, so what is here is unknown rather than absent. */
  readonly failed: boolean;
  /** A fetch is running, whether this screen started it or joined one. */
  readonly refreshing: boolean;
  /** The last refresh could not be started. */
  readonly refreshError: IpcError | null;
  /** Fills the rails from the configured providers and re-reads when it stops. */
  readonly refresh: () => Promise<void>;
}

/** The rails, and the refresh that fills them. */
export function useHomeRails(): HomeRailsView {
  const [state, setState] = useState<{
    rails: readonly HomeRailDto[];
    loading: boolean;
    failed: boolean;
    refreshing: boolean;
    refreshError: IpcError | null;
  }>({ rails: [], loading: true, failed: false, refreshing: false, refreshError: null });

  // The journal row being followed, and how many fetches have stopped. The counter is a re-read
  // trigger rather than a status: a screen re-reads when something *changes*, and a boolean would
  // re-read once and then never again for the second fetch.
  const [follow, setFollow] = useState<RailsFollow>(NOT_FOLLOWING);
  // The follow, where the event listener reads it: an event that arrives before React has committed
  // the state would otherwise be dropped, and the screen would wait for a stop that already happened.
  const following = useRef(follow);

  const applyFollow = useCallback((next: RailsFollow): void => {
    following.current = next;
    setFollow(next);
  }, []);

  const read = useCallback(async (): Promise<void> => {
    try {
      const rails = await homeRails();
      setState((previous) => ({ ...previous, rails, loading: false, failed: false }));
    } catch (caught: unknown) {
      const error = normalizeIpcError(caught);
      logUiError("homeRails", error);
      setState((previous) => ({ ...previous, loading: false, failed: true }));
    }
  }, []);

  const refresh = useCallback(async (): Promise<void> => {
    setState((previous) => ({ ...previous, refreshError: null }));
    try {
      const started = await refreshRails();
      // `coalesced` is the core saying a fetch was already under way: the journal row is the one to
      // follow either way, and the screen says the same thing to the user.
      applyFollow({ journalId: started.journalId, finished: following.current.finished });
      setState((previous) => ({ ...previous, refreshing: true }));
    } catch (caught: unknown) {
      const error = normalizeIpcError(caught);
      logUiError("refreshRails", error);
      setState((previous) => ({ ...previous, refreshError: error }));
    }
  }, [applyFollow]);

  useEffect(() => {
    void read();
  }, [read, follow.finished]);

  useEffect(() => {
    let cancelled = false;
    let unlisten: UnlistenFn | null = null;

    void listen<unknown>(eventTopics.jobState.name, (event) => {
      const next = followRails(following.current, event.payload);
      if (next === following.current) {
        return;
      }
      applyFollow(next);
      setState((previous) => ({ ...previous, refreshing: false }));
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
  }, [applyFollow]);

  return { ...state, refresh };
}
