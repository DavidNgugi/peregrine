/**
 * The hook behind Discover's search box: **the catalogue first, the providers only when it has nothing.**
 *
 * The first version of this asked a provider on every submit, which is the wrong order. A provider
 * request leaves the machine, costs a rate-limit token and a round trip, and is *worse* than the local
 * answer for anything the catalogue has already seen — so the cheap, instant, private read goes first,
 * and the expensive one is what happens when the cheap one comes back empty. That is the whole rule,
 * and it is why the hook reports [`SearchSource`]: a screen that could not say which of the two
 * answered would be showing a provider's spelling of a title it already holds.
 *
 * **Typing searches; it is no longer submitted.** Two characters is the floor — one character matches
 * most of a catalogue and asks a provider for a word — and the typing is debounced, so a five-letter
 * word is one query rather than five. The local read is cheap but it is not free: it is a database
 * query on every keystroke otherwise, and the search path is where `AGENTS §6`'s `≤ 16 ms p50` budget
 * is measured.
 *
 * **Enter is the escape hatch.** The debounce waits; a reader who has decided the catalogue does not
 * have it should not have to wait for a timer, so [`DiscoverSearch::searchNow`] runs the same rule
 * immediately — and cancels the timer rather than racing it.
 *
 * The three states are kept apart for the reason the library's are: still searching, failed, and
 * genuinely empty mean different things to a user, and the middle one must never look like the last.
 */

import { normalizeIpcError, searchProviders, searchTitles } from "@peregrine/ipc-client";
import type { TitleMatchDto, RailKindDto } from "@peregrine/ipc-client";
import { useCallback, useEffect, useRef, useState } from "react";
import { logUiError } from "../../../lib/error-log.js";

/**
 * The fewest characters worth asking anything about.
 *
 * One character is not a query: it matches a large fraction of any catalogue, and it spends a provider
 * request on a word. Two is where a person has committed to something.
 */
export const MIN_QUERY_CHARS = 2;

/** How long typing has to stop before a search is worth making. */
export const SEARCH_DEBOUNCE_MS = 250;

/**
 * Where what is on screen came from.
 *
 * `"catalogue"` is the local index — instant, offline, and nothing left the machine. `"provider"` means
 * the local read found nothing and a provider was asked, so these are titles the core has just written
 * into the catalogue.
 */
export type SearchSource = "none" | "catalogue" | "provider";

/** What a search found, and what the screen needs to know about it. */
export interface DiscoverSearch {
  /** The text in the box, owned here so the debounce has one place to live. */
  readonly query: string;
  readonly setQuery: (query: string) => void;
  /** What the catalogue holds, or what a provider answered when the catalogue held nothing. */
  readonly results: readonly TitleMatchDto[];
  /** Which of the two answered, which is what the screen says above the results. */
  readonly source: SearchSource;
  /** A read is in flight. */
  readonly searching: boolean;
  /** The last read failed, so the results are unknown rather than empty. */
  readonly failed: boolean;
  /** Whether a search has been made at all, which is what tells an empty list from an empty screen. */
  readonly searched: boolean;
  /** Runs the search now rather than after the debounce: Enter, and the magnifier. */
  readonly searchNow: () => void;
}

/** The state of one Discover search. */
interface SearchState {
  readonly results: readonly TitleMatchDto[];
  readonly source: SearchSource;
  readonly searching: boolean;
  readonly failed: boolean;
  readonly searched: boolean;
  readonly kind: RailKindDto | undefined;
}

const EMPTY: Omit<SearchState, "kind"> = {
  results: [],
  source: "none",
  searching: false,
  failed: false,
  searched: false,
};

/** The state one search settles into, so the two answers are written once. */
function settled(
  kind: RailKindDto | undefined,
  results: readonly TitleMatchDto[],
  source: SearchSource,
): SearchState {
  return { kind, results, source, searching: false, failed: false, searched: true };
}

/** The hook behind Discover's search box. */
export function useDiscoverSearch(kind?: RailKindDto): DiscoverSearch {
  const [query, setQuery] = useState("");
  const [state, setState] = useState<SearchState>({ kind, ...EMPTY });
  // Which request the answers belong to: a read that lands after the text moved on is dropped, and an
  // empty box cancels whatever is in the air rather than letting it fill the panel behind a cleared
  // field.
  const version = useRef(0);
  // The pending debounce, so `searchNow` can run instead of waiting for it rather than beside it.
  const pending = useRef<ReturnType<typeof setTimeout> | null>(null);

  const run = useCallback(async (text: string, selectedKind: RailKindDto | undefined) => {
    const trimmed = text.trim();
    if (trimmed.length < MIN_QUERY_CHARS) {
      version.current += 1;
      setState({ kind: selectedKind, ...EMPTY });
      return;
    }
    const request = ++version.current;
    setState((current) => ({ ...current, kind: selectedKind, searching: true, failed: false }));
    try {
      // **Cache first.** The catalogue's own index answers without leaving the machine, so anything the
      // reader has already synced is found here and costs no request (`AGENTS R24`).
      const local = await searchTitles(trimmed);
      if (request !== version.current) return;
      if (local.length > 0) {
        setState(settled(selectedKind, local, "catalogue"));
        return;
      }
      // Nothing here, so this is one of the asks the providers exist for.
      const found = await searchProviders(trimmed, undefined, selectedKind);
      if (request !== version.current) return;
      setState(settled(selectedKind, found, "provider"));
    } catch (error: unknown) {
      if (request !== version.current) return;
      logUiError("discover.search", normalizeIpcError(error));
      setState((current) => ({ ...current, searching: false, failed: true, searched: true }));
    }
  }, []);

  useEffect(() => {
    // A box too short to search is answered at once rather than after a wait: there is nothing to ask,
    // and the panel should close as the reader deletes rather than linger over an abandoned word.
    if (query.trim().length < MIN_QUERY_CHARS) {
      void run(query, kind);
      return;
    }
    pending.current = setTimeout(() => {
      pending.current = null;
      void run(query, kind);
    }, SEARCH_DEBOUNCE_MS);
    return () => {
      if (pending.current !== null) {
        clearTimeout(pending.current);
        pending.current = null;
      }
    };
  }, [query, kind, run]);

  const searchNow = useCallback((): void => {
    if (pending.current !== null) {
      clearTimeout(pending.current);
      pending.current = null;
    }
    void run(query, kind);
  }, [query, kind, run]);

  return {
    // A search for another kind is not an answer about this one, for the reason the rails read gives.
    ...(state.kind === kind ? state : { kind, ...EMPTY }),
    query,
    setQuery,
    searchNow,
  };
}
