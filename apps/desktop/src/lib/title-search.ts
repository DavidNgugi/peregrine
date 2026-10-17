/**
 * The hook behind a title search box: **the catalogue first, the providers when it has nothing — or
 * the providers first when the caller is searching everything.**
 *
 * The first version of this asked a provider on every submit, which is the wrong order for the
 * screen it was written for. A provider request leaves the machine, costs a rate-limit token and a
 * round trip, and is *worse* than the local answer for anything the catalogue has already seen — so
 * Discover's dropdown asks the cheap, instant, private read first and spends the expensive one only
 * when the cheap one comes back empty. That is why the hook reports [`SearchSource`]: a screen that
 * could not say which of the two answered would be showing a provider's spelling of a title it
 * already holds.
 *
 * **Browse uses tile mode, and the reason is a real search defect rather than a preference.**
 * The catalogue's index tokenises by word: "Spider-Man" is `spider` and `man`, so a reader who
 * types "spiderman" has no term the index can match, even with the title sitting on the screen in
 * front of them. A provider knows the name it publishes, so a search that is *about finding a title*
 * asks it. [`useTitleSearch`]'s tile mode calls `search_tiles`, which writes the provider's answer
 * through the same list writer a browse page uses — so the result carries the year, the score, the
 * synopsis, and the cached poster, and a poster grid draws it immediately.
 *
 * **Typing searches; it is no longer submitted.** Two characters is the floor — one character
 * matches most of a catalogue and asks a provider for a word — and the typing is debounced, so a
 * five-letter word is one query rather than five. Discover's local read is cheap but not free: it
 * is a database query on every keystroke otherwise, and the search path is where `AGENTS §6`'s
 * `≤ 16 ms p50` budget is measured. A provider-backed search needs the pause even more, because
 * every one of them is a request.
 *
 * **Enter is the escape hatch.** The debounce waits; a reader who has decided the providers do not
 * have it should not have to wait for a timer, so [`TitleSearch::searchNow`] runs the same rule
 * immediately — and cancels the timer rather than racing it.
 *
 * The three states are kept apart for the reason the library's are: still searching, failed, and
 * genuinely empty mean different things to a user, and the middle one must never look like the last.
 */

import {
  normalizeIpcError,
  searchProviders,
  searchTiles,
  searchTitles,
} from "@peregrine/ipc-client";
import type { RailItemDto, RailKindDto, TitleMatchDto } from "@peregrine/ipc-client";
import { useCallback, useEffect, useRef, useState } from "react";
import { logUiError } from "./error-log.js";

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
 * a provider was asked, so these are titles the core has written into the catalogue.
 */
export type SearchSource = "none" | "catalogue" | "provider";

/** What every search surface holds: the text, the three states, and the escape hatch. */
interface SearchBase {
  /** The text in the box, owned here so the debounce has one place to live. */
  readonly query: string;
  readonly setQuery: (query: string) => void;
  /** A read is in flight. */
  readonly searching: boolean;
  /** The last read failed, so the results are unknown rather than empty. */
  readonly failed: boolean;
  /** Whether a search has been made at all, which is what tells an empty list from an empty screen. */
  readonly searched: boolean;
  /** Runs the search now rather than after the debounce: Enter, and a magnifier. */
  readonly searchNow: () => void;
}

/** A text search: the catalogue's identities, and which reader answered. */
export interface TitleSearch extends SearchBase {
  /** What the catalogue holds, or what a provider answered when the catalogue held nothing. */
  readonly results: readonly TitleMatchDto[];
  /** Which of the two answered, which is what the screen says above the results. */
  readonly source: SearchSource;
}

/** A tile search: what a poster grid draws, written through the list path. */
export interface TileSearch extends SearchBase {
  /** The matches, with the year, score, synopsis, and cached poster path a tile needs. */
  readonly results: readonly RailItemDto[];
}

/** How a caller wants a text question asked. */
export interface TitleSearchOptions {
  /**
   * Ask the providers first, instead of the catalogue first.
   *
   * `true` is for a search that is about finding a title rather than narrowing what is here: the
   * catalogue's index cannot match a compound the reader typed without its punctuation, and the
   * provider that published the title can. `false` — the default, and Discover's choice — keeps
   * the local answer free and offline whenever there is one.
   */
  readonly providers?: boolean;
}

/** How a caller asks for the tile-shaped answer. */
export interface TileSearchOptions {
  readonly tiles: true;
}

/** The state of one search, whichever shape its results take. */
interface SearchState {
  readonly results: readonly (TitleMatchDto | RailItemDto)[];
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

/** The state one search settles into, so the answers are written once. */
function settled(
  kind: RailKindDto | undefined,
  results: readonly (TitleMatchDto | RailItemDto)[],
  source: SearchSource,
): SearchState {
  return { kind, results, source, searching: false, failed: false, searched: true };
}

/**
 * The hook behind a title search box.
 *
 * `{ tiles: true }` answers with [`TileSearch`] — the poster fields a grid draws — and every other
 * option answers with [`TitleSearch`]. The overloads are the two contracts; the state machine behind
 * them is one debounce, one request generation, and one set of three answers.
 */
export function useTitleSearch(
  kind: RailKindDto | undefined,
  options: TileSearchOptions,
): TileSearch;
export function useTitleSearch(kind?: RailKindDto, options?: TitleSearchOptions): TitleSearch;
export function useTitleSearch(
  kind: RailKindDto | undefined,
  options: TitleSearchOptions | TileSearchOptions = {},
): TitleSearch | TileSearch {
  const tiles = "tiles" in options && options.tiles === true;
  const providersFirst = !tiles && "providers" in options && options.providers === true;
  const [query, setQuery] = useState("");
  const [state, setState] = useState<SearchState>({ kind, ...EMPTY });
  // Which request the answers belong to: a read that lands after the text moved on is dropped, and an
  // empty box cancels whatever is in the air rather than letting it fill the panel behind a cleared
  // field.
  const version = useRef(0);
  // The pending debounce, so `searchNow` can run instead of waiting for it rather than beside it.
  const pending = useRef<ReturnType<typeof setTimeout> | null>(null);

  const run = useCallback(
    async (text: string, selectedKind: RailKindDto | undefined) => {
      const trimmed = text.trim();
      if (trimmed.length < MIN_QUERY_CHARS) {
        version.current += 1;
        setState({ kind: selectedKind, ...EMPTY });
        return;
      }
      const request = ++version.current;
      setState((current) => ({ ...current, kind: selectedKind, searching: true, failed: false }));
      try {
        if (tiles) {
          // The provider is asked directly, and the core writes its answer through the list writer
          // before returning it, so a result is a catalogue row with the poster a grid draws.
          const found = await searchTiles(trimmed, undefined, selectedKind);
          if (request !== version.current) return;
          setState(settled(selectedKind, found, "provider"));
          return;
        }
        if (providersFirst) {
          // The provider is asked directly, and the core writes its answer before returning it, so a
          // result is a title the catalogue owns. The local index is not consulted first: it cannot
          // match the compound the provider just found, and the point of this mode is that it does.
          const found = await searchProviders(trimmed, undefined, selectedKind);
          if (request !== version.current) return;
          setState(settled(selectedKind, found, "provider"));
          return;
        }
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
        logUiError("title.search", normalizeIpcError(error));
        setState((current) => ({ ...current, searching: false, failed: true, searched: true }));
      }
    },
    [providersFirst, tiles],
  );

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

  // A search for another kind is not an answer about this one, for the reason the rails read gives.
  const active = state.kind === kind ? state : { kind, ...EMPTY };
  const base = {
    query,
    setQuery,
    searching: active.searching,
    failed: active.failed,
    searched: active.searched,
    searchNow,
  };
  return tiles
    ? { ...base, results: active.results as readonly RailItemDto[] }
    : {
        ...base,
        results: active.results as readonly TitleMatchDto[],
        source: active.source,
      };
}
