/**
 * The hook behind Discover: asking the providers, then showing what the catalogue holds.
 *
 * One call per search rather than one per keystroke. A search leaves the machine — it asks a provider,
 * waits for an answer, and writes what came back — so typing must not spend a request per character, and
 * the screen submits instead. That is a deliberate difference from the library's local search, which is
 * cheap enough to run as the user types.
 *
 * The three states are kept apart for the reason the library's are: still searching, failed, and
 * genuinely empty mean different things to a user, and the middle one must never look like the last.
 */

import { normalizeIpcError, searchProviders } from "@peregrine/ipc-client";
import type { TitleMatchDto } from "@peregrine/ipc-client";
import { useCallback, useState } from "react";
import { logUiError } from "../../../lib/error-log.js";

/** What a search found, and what the screen needs to know about it. */
export interface DiscoverSearch {
  /** What the catalogue holds for the last query searched. */
  readonly results: readonly TitleMatchDto[];
  /** A search is in flight. */
  readonly searching: boolean;
  /** The last search failed, so the results are unknown rather than empty. */
  readonly failed: boolean;
  /** Whether a search has been made at all, which is what tells an empty list from an empty screen. */
  readonly searched: boolean;
  /** Searches the providers, then re-reads the catalogue. */
  readonly search: (query: string) => Promise<void>;
}

/** The state of one Discover search. */
export function useDiscoverSearch(): DiscoverSearch {
  const [state, setState] = useState<Omit<DiscoverSearch, "search">>({
    results: [],
    searching: false,
    failed: false,
    searched: false,
  });

  const search = useCallback(async (query: string) => {
    const trimmed = query.trim();
    if (trimmed === "") {
      return;
    }
    setState((current) => ({ ...current, searching: true, failed: false }));
    try {
      const results = await searchProviders(trimmed);
      setState({ results, searching: false, failed: false, searched: true });
    } catch (error: unknown) {
      logUiError("discover.search", normalizeIpcError(error));
      setState((current) => ({ ...current, searching: false, failed: true, searched: true }));
    }
  }, []);

  return { ...state, search };
}
