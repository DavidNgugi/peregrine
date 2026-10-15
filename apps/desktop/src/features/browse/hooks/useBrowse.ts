/**
 * The data layer behind Browse: one page of a queried list, and the next page when the grid asks.
 *
 * **A page at a time, because that is what a provider answers and what a grid can draw.** The core's
 * `browse` command returns the provider's own page plus `hasMore`, so this hook holds the pages it
 * has received and appends; it never asks for a page it already has, and it never guesses whether
 * another one exists (`AGENTS R7` — that decision is the provider's and travels on the answer).
 *
 * **Every filter change starts again from page one, and a page in the air belongs to the request
 * that asked for it.** A page that lands after the filters moved is dropped rather than appended:
 * otherwise narrowing to "documentaries over 8" would show the previous list's tail under the new
 * heading. The generation counter is that rule, and the in-flight flag is its sibling — the grid
 * calls `loadMore` on every render near the end, so without it a slow page would be requested once
 * per render.
 *
 * **A failure is reported, not thrown away.** The list keeps what it has and says so, because a
 * provider having a bad minute is a reason to say "that page did not load" rather than to blank a
 * grid the reader was looking at.
 */

import { browse, normalizeIpcError } from "@peregrine/ipc-client";
import type { BrowseSortDto, IpcError, RailItemDto } from "@peregrine/ipc-client";
import { useCallback, useEffect, useRef, useState } from "react";
import { logUiError } from "../../../lib/error-log.js";
import type { TitleKind } from "../../../app/routes.js";
import { mayLoad, mergePage, stillHasMore } from "../../../lib/paged-list.js";

/** What the reader narrowed the list to. */
export interface BrowseFilters {
  /** The provider's own genre identifier, or `null` for every genre. */
  readonly genre: number | null;
  /** How the provider is asked to order the page. */
  readonly sort: BrowseSortDto;
  /** The lowest score out of ten a title may have, or `null` for no floor. */
  readonly minRating: number | null;
}

/** What the grid starts on: every genre, the provider's own order, no floor. */
export const DEFAULT_FILTERS: BrowseFilters = { genre: null, sort: "popularity", minRating: null };

/** What the Browse screen renders and what it needs to know about the reads. */
export interface BrowseData {
  /** Every page received so far, in the provider's order. */
  readonly items: readonly RailItemDto[];
  /** The filters in force, which the toolbar draws. */
  readonly filters: BrowseFilters;
  /**
   * The identity of the query `items` answers.
   *
   * It changes when the query changes and not when a page is appended, which is what the grid's
   * `resetKey` needs: a replaced list starts at the top, an appended one must not.
   */
  readonly queryKey: string;
  /** The provider said another page exists. */
  readonly hasMore: boolean;
  /** A page is in flight. */
  readonly loading: boolean;
  /** The last page failed, so what is here is short rather than complete. */
  readonly failed: IpcError | null;
  /** Applies a filter change and starts again from the first page. */
  readonly setFilters: (filters: BrowseFilters) => void;
  /** Asks for the next page, when there is one and none is in flight. */
  readonly loadMore: () => void;
}

/** Everything the hook holds about the read. */
interface BrowseState {
  readonly items: readonly RailItemDto[];
  readonly hasMore: boolean;
  readonly loading: boolean;
  readonly failed: IpcError | null;
}

const EMPTY: BrowseState = { items: [], hasMore: false, loading: true, failed: null };

/**
 * One page at a time of `kind`'s catalogue, narrowed by the filters the screen holds.
 *
 * `kind` is the route's, so switching between Films and TV shows is a navigation rather than a piece
 * of state this hook owns — and a change here restarts the list, because a page of films is not a
 * page of series.
 */
export function useBrowse(kind: TitleKind, initial?: BrowseFilters): BrowseData {
  // The starting filters are a parameter because a *shelf* is a browse read with the query already
  // decided: a genre shelf under a title's recommendations is this hook with one genre and no control
  // over it, and a hook that always began at `DEFAULT_FILTERS` would spend its first request on the
  // unfiltered list and its second on the one the caller wanted.
  const [filters, setFiltersState] = useState<BrowseFilters>(initial ?? DEFAULT_FILTERS);
  const [state, setState] = useState<BrowseState>(EMPTY);

  // The request in flight, so the grid's every-render `loadMore` asks for one page and not twenty.
  const inFlight = useRef(false);
  // Which request the answers belong to. A filter change moves it, and an answer from an older
  // generation is dropped rather than appended to a list it does not describe.
  const generation = useRef(0);
  // The page the next request asks for: one after a filter change, one more after each page.
  const next = useRef(1);

  const load = useCallback(
    async (page: number, replacing: boolean): Promise<void> => {
      const mine = generation.current;
      inFlight.current = true;
      setState((previous) => ({ ...previous, loading: true, failed: null }));
      try {
        const answer = await browse(
          kind,
          page,
          filters.sort,
          filters.genre ?? undefined,
          filters.minRating ?? undefined,
        );
        if (mine !== generation.current) {
          return;
        }
        setState((previous) => ({
          items: mergePage(previous.items, answer.items, replacing),
          hasMore: stillHasMore(answer.items.length, answer.hasMore),
          loading: false,
          failed: null,
        }));
        next.current = page + 1;
      } catch (caught: unknown) {
        if (mine !== generation.current) {
          return;
        }
        const error = normalizeIpcError(caught);
        logUiError("browse", error);
        setState((previous) => ({ ...previous, loading: false, failed: error }));
      } finally {
        // Only this generation may clear the flag: a change that arrived while this page was in the
        // air has already set it for the request it started.
        if (mine === generation.current) {
          inFlight.current = false;
        }
      }
    },
    [kind, filters],
  );

  // **A new query is a new generation, and that is what makes a tab switch safe.** `load` is a new
  // function for a filter change and for a different kind, so this one effect covers both: it moves
  // the generation — a page still in the air belongs to the query just left, and appending it would
  // put films under a series heading — rewinds to page one, clears what the previous query drew, and
  // asks for the first page of the new one. The reset lives *here* rather than in `setFilters`
  // because a kind change comes from the route and never passes through that callback at all.
  useEffect(() => {
    generation.current += 1;
    inFlight.current = false;
    next.current = 1;
    setState({ items: [], hasMore: false, loading: true, failed: null });
    void load(1, true);
  }, [load]);

  const setFilters = useCallback((wanted: BrowseFilters): void => {
    setFiltersState(wanted);
  }, []);

  const loadMore = useCallback((): void => {
    if (!mayLoad(inFlight.current, state.loading, state.hasMore)) {
      return;
    }
    void load(next.current, false);
  }, [load, state.hasMore, state.loading]);

  return {
    ...state,
    filters,
    // The identity of the query the items answer, which is what tells the grid that the list was
    // *replaced* rather than appended to — so it can start a new answer from the top instead of
    // drawing page one from wherever the previous query was scrolled to.
    queryKey: `${kind}:${String(filters.genre)}:${filters.sort}:${String(filters.minRating)}`,
    setFilters,
    loadMore,
  };
}
