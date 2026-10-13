/**
 * The hook layer for the picker: searching the catalogue for a title to file a file under.
 *
 * A read per keystroke, with no debounce, and that is a decision rather than an omission: the search
 * is local `FTS5` against an index the core maintains, budgeted at 16 ms p50 (§6), and a debounce
 * would be a timer between the user's intent and the answer for no measured gain. When a remote
 * fallback exists (`PG-E13-*`) the debounce belongs to *that* path, not this one.
 *
 * The states are the usual three, plus `idle`: an empty query is not a search, and a picker that said
 * "nothing found" before anything was typed would be answering a question nobody asked.
 */

import { normalizeIpcError, searchTitles } from "@peregrine/ipc-client";
import type { IpcError, TitleMatchDto } from "@peregrine/ipc-client";
import { useCallback, useEffect, useState } from "react";
import { logUiError } from "../../../lib/error-log.js";

/** What the picker needs to know about one search. */
export interface TitleSearch {
  /** What the last query found, in the core's own order. */
  readonly results: readonly TitleMatchDto[];
  /** The query has not been typed yet. */
  readonly idle: boolean;
  /** A search is in flight. */
  readonly searching: boolean;
  /** The search failed, so the list is unknown rather than empty. */
  readonly failed: boolean;
}

/** Titles matching `query`, read as it is typed. */
export function useTitleSearch(query: string): TitleSearch {
  const [state, setState] = useState<Omit<TitleSearch, "idle">>({
    results: [],
    searching: false,
    failed: false,
  });

  const read = useCallback(async (text: string): Promise<void> => {
    if (text.trim() === "") {
      setState({ results: [], searching: false, failed: false });
      return;
    }
    setState((previous) => ({ ...previous, searching: true, failed: false }));
    try {
      const results = await searchTitles(text);
      setState({ results, searching: false, failed: false });
    } catch (caught: unknown) {
      const error: IpcError = normalizeIpcError(caught);
      logUiError("searchTitles", error);
      setState({ results: [], searching: false, failed: true });
    }
  }, []);

  useEffect(() => {
    void read(query);
  }, [query, read]);

  return { ...state, idle: query.trim() === "" };
}
