/**
 * The titles a provider considers like the one on screen.
 *
 * **A read of the title, not of the screen.** It is asked once per visit — the id is the route's —
 * and it answers empty for a title no configured provider holds an identifier for, which is a real
 * answer rather than a failure: a title the library inferred from a filename has no recommendations
 * to fetch, and the page draws no rail for it instead of an error beside the episodes.
 *
 * A failure is logged and drawn as no rail, for the same reason: the page's own answer — the
 * library's counts and the episodes — does not depend on this read, and replacing it with an error
 * because a recommendation did not arrive would tell the reader the title is missing.
 */

import { normalizeIpcError, recommendations } from "@peregrine/ipc-client";
import type { GenreDto, RailItemDto, RecommendationsDto } from "@peregrine/ipc-client";
import { useEffect, useState } from "react";
import { logUiError } from "../../../lib/error-log.js";

/** One title's recommendations, and what the drawing needs to know about the read. */
export interface SimilarTitles {
  /** The titles, in the provider's order; empty while the read is in the air and when it finds none. */
  readonly items: readonly RailItemDto[];
  /**
   * The genres the rail's titles share, most-shared first and already named.
   *
   * **Read off the rail, not off the title**, because nothing in this build stores a title's own
   * genres: the shelves these become are "more of what the provider listed around this title", which
   * for a franchise or a genre picture is the same answer. Empty means the provider named no genres, so
   * there are no shelves — not that the title has none.
   */
  readonly genres: readonly GenreDto[];
}

/**
 * The titles like `id`, or nothing while there is no title on screen.
 *
 * `null` is "no title route is open", which the shell is in on every other screen: the hook cannot be
 * called conditionally — the rules of hooks — so the read is what is conditional.
 */
export function useSimilarTitles(id: number | null): SimilarTitles {
  const [found, setFound] = useState<RecommendationsDto>({ items: [], genres: [] });

  useEffect(() => {
    if (id === null) {
      // A route that is not a title forgets what the last one found: coming back to it is a new
      // visit, and a rail left over from another title's page would be another title's answer.
      setFound({ items: [], genres: [] });
      return;
    }
    let cancelled = false;
    void (async () => {
      try {
        const answer = await recommendations(id);
        if (!cancelled) {
          setFound(answer);
        }
      } catch (caught: unknown) {
        if (!cancelled) {
          logUiError("recommendations", normalizeIpcError(caught));
          setFound({ items: [], genres: [] });
        }
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [id]);

  return { items: found.items, genres: found.genres };
}
