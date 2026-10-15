/**
 * The data layer behind Discover: the rails the core already holds, and the job that fills them.
 *
 * **The read is cache-first and the screen paints from it** (`FLOWS F2`): `homeRails` answers from
 * the catalogue and the artwork cache and never waits for a provider, so a cold start draws every
 * rail's heading and its empty state before any request has left the machine. The fetch is a job
 * whose terminal phase triggers one re-read — no polling, no spinner (`DESIGN_SYSTEM §11` rule 2).
 *
 * **The read is also what starts the first fetch**, in the core (`FLOWS F2`), so a screen with a
 * configured provider fills without anyone pressing Refresh; what the read answers is the state the
 * screen says out loud. That fetch's row is not in the answer — the read gives the state, not the
 * journal id — so it is watched by *kind* rather than followed by id, and once per visit: a re-read
 * that still finds every rail empty starts another fetch, and watching for ever would be a loop.
 *
 * **A Refresh this screen pressed is followed by its *journal* row and not by its scheduler id.** A
 * submission that coalesced onto a fetch already running has no scheduler id of its own
 * (`peregrine_app::StartedJob::job` is nil for one), and every `job://state` payload carries both
 * ids — so the journal id is the one a screen can always match on. That is what makes two presses
 * of Refresh, or two visits to this screen, land on one fetch and one re-read.
 *
 * Every event is read through `lib/job-events.ts`, which is where the envelope rule lives, so this
 * hook and the library scan cannot disagree about which payloads are readable.
 */

import { eventTopics, homeRails, normalizeIpcError, refreshRails } from "@peregrine/ipc-client";
import type { HomeRailDto, IpcError, RailKindDto } from "@peregrine/ipc-client";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";
import { logUiError } from "../../../lib/error-log.js";
import { followRails, NOT_FOLLOWING, railsFetchStopped, type RailsFollow } from "./rails-follow.js";

/**
 * What the core says the rails are doing: no key set up, a fetch in the air, or rails to draw.
 *
 * Named through the rail it arrives on because the package's hand-written index re-exports the rail
 * DTO and not every enum the generated contract declares — and naming it that way leaves the
 * generated bindings as the only file a regeneration has to touch.
 */
type RailsState = HomeRailDto["state"];

/** What the Discover screen renders and what it needs to know about the reads. */
export interface HomeRailsView {
  /** The rails, in the order the core lists them. Empty until the first read answers. */
  readonly rails: readonly HomeRailDto[];
  /**
   * What the core says the rails are doing: nothing can fill them without a provider key, the first
   * fetch is in the air, or they are here. Rendered rather than derived (`AGENTS R7`) — an empty
   * rail list is what the first two both look like from here.
   */
  readonly state: RailsState;
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
export function useHomeRails(kind?: RailKindDto): HomeRailsView {
  const [state, setState] = useState<{
    kind: RailKindDto | undefined;
    rails: readonly HomeRailDto[];
    state: RailsState;
    loading: boolean;
    failed: boolean;
    refreshing: boolean;
    refreshError: IpcError | null;
  }>({
    kind,
    rails: [],
    // Before any answer the screen draws the empty state it always drew: `ready` is the state that
    // does, and a read in flight is not a state this screen draws (`DESIGN_SYSTEM §11` rule 2).
    state: "ready",
    loading: true,
    failed: false,
    refreshing: false,
    refreshError: null,
  });

  // The journal row being followed, and how many fetches have stopped. The counter is a re-read
  // trigger rather than a status: a screen re-reads when something *changes*, and a boolean would
  // re-read once and then never again for the second fetch.
  const [follow, setFollow] = useState<RailsFollow>(NOT_FOLLOWING);
  // The follow, where the event listener reads it: an event that arrives before React has committed
  // the state would otherwise be dropped, and the screen would wait for a stop that already happened.
  const following = useRef(follow);
  // Whether the fetch a read started has already been watched stop. One per visit is the whole of
  // it: a read that still finds every rail empty starts another fetch, so re-reading for ever would
  // be a fetch loop against a provider with nothing to give.
  const readFetchFinished = useRef(false);

  const applyFollow = useCallback((next: RailsFollow): void => {
    following.current = next;
    setFollow(next);
  }, []);

  const readVersion = useRef(0);
  const read = useCallback(async (): Promise<void> => {
    const version = ++readVersion.current;
    try {
      const rails = await homeRails(kind);
      if (version !== readVersion.current) return;
      setState((previous) => ({
        ...previous,
        // The state is the core's answer, carried on every rail because the answer is a list. The
        // fallback keeps the previous answer rather than inventing one, for a list the core always
        // fills (`Runtime::home_rails` answers every rail, which the core's own test pins).
        state: rails[0]?.state ?? previous.state,
        rails,
        kind,
        loading: false,
        failed: false,
      }));
    } catch (caught: unknown) {
      if (version !== readVersion.current) return;
      const error = normalizeIpcError(caught);
      logUiError("homeRails", error);
      setState((previous) => ({ ...previous, loading: false, failed: true }));
    }
  }, [kind]);

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
      if (next !== following.current) {
        applyFollow(next);
        setState((previous) => ({ ...previous, refreshing: false }));
        return;
      }
      // A fetch this screen did not submit is the one a *read* started (`FLOWS F2`), and its
      // completion is what tells the screen to read again — the row it holds is not in the answer,
      // so it cannot be followed by id the way a Refresh is. Once per visit, because the re-read of
      // an empty answer starts another fetch.
      if (readFetchFinished.current || !railsFetchStopped(event.payload)) {
        return;
      }
      readFetchFinished.current = true;
      applyFollow({ journalId: null, finished: following.current.finished + 1 });
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

  return {
    ...state,
    rails: state.kind === kind ? state.rails : [],
    loading: state.kind !== kind || state.loading,
    refresh,
  };
}
