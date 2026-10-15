/**
 * The endless feed under a title: the recommendations first, then the catalogue's own pages of the
 * genres they share.
 *
 * **One feed rather than a rail plus a shelf per genre, and that is a correction.** The first shape was
 * a horizontal rail of recommendations with a separate vertical shelf under it for each shared genre,
 * which gave the page three scroll areas stacked on each other and — because two identifiers can resolve
 * to the same display name — could draw the same genre twice, which a reader saw as two identical
 * "More Animation" sections. One queue of sources cannot do either: every title is appended to one list
 * and a duplicate is dropped where it would have been drawn (`mergePage`), and there is exactly one
 * place for the reader to scroll.
 *
 * **The sources are drained in order, one page at a time.** The recommendations arrive already read (by
 * `useSimilarTitles`, which is also what decides the genres), so they are the head of the feed and cost
 * nothing here. Each genre is then a browse read with its query already decided: page one, page two, and
 * so on until the provider says there is no more, at which point the queue moves to the next genre. The
 * reader sees one list that never ends until the provider has nothing left to offer.
 *
 * **A failure mid-feed is reported and the feed is left standing.** What is already on screen is the
 * reader's, and a page that did not arrive is one page short rather than wrong.
 */

import { browse, normalizeIpcError } from "@peregrine/ipc-client";
import type { GenreDto, IpcError, RailItemDto } from "@peregrine/ipc-client";
import { useCallback, useEffect, useRef, useState } from "react";

import type { TitleKind } from "../../../app/routes.js";
import { logUiError } from "../../../lib/error-log.js";
import { mayLoad, mergePage, stillHasMore } from "../../../lib/paged-list.js";

/** One queue of genres the feed has still to draw from. */
interface Cursor {
  /** The genres not yet finished, in the order the core named them (most-shared first). */
  readonly remaining: readonly GenreDto[];
  /** The page to ask the head genre for next, counting from one. */
  readonly page: number;
}

/** What the feed is doing, and how to ask it for more. */
export interface TitleFeed {
  /** Everything drawn so far, in order: the recommendations, then pages of each shared genre. */
  readonly items: readonly RailItemDto[];
  /** A page is in flight. */
  readonly loading: boolean;
  /** The last page failed, so what is here is short rather than complete. */
  readonly failed: IpcError | null;
  /** Every source is drained, so there is nothing more to ask for. */
  readonly exhausted: boolean;
  /** Asks for the next page of the head genre, when there is one. */
  readonly loadMore: () => void;
}

/** The feed's state, all of it, so a reset is one assignment. */
interface FeedState {
  readonly items: readonly RailItemDto[];
  readonly cursor: Cursor;
  readonly loading: boolean;
  readonly failed: IpcError | null;
}

/**
 * The feed for one title.
 *
 * `seed` is what `useSimilarTitles` answered — the recommendations and the genres they share — and it is
 * read as the feed's *head* rather than appended to: a new title is a new feed, and the signature below
 * is what says so without the hook depending on an object identity that changes every render.
 */
export function useTitleFeed(
  kind: TitleKind,
  seed: {
    readonly items: readonly RailItemDto[];
    readonly genres: readonly GenreDto[];
  },
): TitleFeed {
  // A fresh object every render, so the identity cannot be a dependency: what identifies the seed is
  // which title it starts with and which genres it named.
  const signature = `${String(seed.items[0]?.titleId ?? 0)}|${seed.genres
    .map((genre) => genre.id)
    .join(",")}`;

  const [state, setState] = useState<FeedState>({
    items: seed.items,
    cursor: { remaining: seed.genres, page: 1 },
    loading: false,
    failed: null,
  });
  const inFlight = useRef(false);
  const generation = useRef(0);

  // A new title, or the same title with a different genre list, is a new feed.
  useEffect(() => {
    generation.current += 1;
    inFlight.current = false;
    setState({
      items: seed.items,
      cursor: { remaining: seed.genres, page: 1 },
      loading: false,
      failed: null,
    });
    // `seed` is deliberately absent from the dependencies: it is a new object every render, and the
    // signature above is its *content*, which is what this effect is actually about.
  }, [signature]);

  const loadMore = useCallback((): void => {
    const head = state.cursor.remaining[0];
    if (head === undefined || !mayLoad(inFlight.current, state.loading, true)) {
      return;
    }
    const mine = generation.current;
    const page = state.cursor.page;
    inFlight.current = true;
    setState((previous) => ({ ...previous, loading: true, failed: null }));

    void (async () => {
      try {
        const answer = await browse(kind, page, "popularity", head.id, undefined);
        if (mine !== generation.current) {
          return;
        }
        setState((previous) => {
          // The next genre when this one has nothing after it, the next page otherwise.
          const drained = !stillHasMore(answer.items.length, answer.hasMore);
          const remaining = drained
            ? previous.cursor.remaining.slice(1)
            : previous.cursor.remaining;
          return {
            items: mergePage(previous.items, answer.items, false),
            cursor: { remaining, page: drained ? 1 : previous.cursor.page + 1 },
            loading: false,
            failed: null,
          };
        });
      } catch (caught: unknown) {
        if (mine !== generation.current) {
          return;
        }
        const error = normalizeIpcError(caught);
        logUiError("title.feed", error);
        setState((previous) => ({ ...previous, loading: false, failed: error }));
      } finally {
        if (mine === generation.current) {
          inFlight.current = false;
        }
      }
    })();
  }, [kind, state.cursor, state.loading]);

  return {
    ...state,
    exhausted: state.cursor.remaining.length === 0,
    loadMore,
  };
}
