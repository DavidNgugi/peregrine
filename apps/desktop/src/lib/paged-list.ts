/**
 * The rules a paged list follows, as pure functions.
 *
 * **They live in `lib/` rather than inside a hook, for two reasons.** The first is that a hook cannot
 * be tested here: this repository renders to static markup in a Node environment with no DOM
 * (`vitest.config.ts`), so a hook's effects never run in a test — which is why `useHomeRails` keeps its
 * follow logic in `rails-follow.ts` and `useTitle` keeps its refresh rule in `refresh-follow.ts`. The
 * second is that *two* features now page a list: the browse screen, and the endless feed on a title's
 * page that appends genre pages under the recommendations. A rule two features share lives between
 * them (`AGENTS §5`) rather than in whichever one happened to need it first.
 */

import type { RailItemDto } from "@peregrine/ipc-client";

/**
 * One title, once.
 *
 * **A provider's pages overlap, and it is not a defect.** `sort_by=popularity.desc` is ordered by a
 * number that moves while a reader scrolls: a title on page one can slide to page two between the
 * two requests, and a title can gain enough votes to appear on both. What the grid does with the
 * second copy is not "draw it again" — its tiles are keyed by the catalogue row, so a repeat is a
 * duplicate React key and two tiles that select as one. The first copy wins, because the first page
 * is the one the provider ranked first.
 */
function once(items: readonly RailItemDto[]): readonly RailItemDto[] {
  const seen = new Set<number>();
  return items.filter((item) => {
    if (seen.has(item.titleId)) {
      return false;
    }
    seen.add(item.titleId);
    return true;
  });
}

/**
 * What one page's arrival leaves on screen.
 *
 * `replacing` is the first page of a filter set — a new query replaces what is drawn rather than
 * growing it — and every page after it appends. The merge is where the overlap rule is applied, so a
 * page that repeats a title already on screen adds a tile nowhere.
 */
export function mergePage(
  previous: readonly RailItemDto[],
  page: readonly RailItemDto[],
  replacing: boolean,
): readonly RailItemDto[] {
  return replacing ? once(page) : once([...previous, ...page]);
}

/**
 * Whether another page may be asked for.
 *
 * Three ways not to: a request is already in the air (the grid asks on every render near the end),
 * the previous one has not settled, or the provider has already said there is no next page. The last
 * is the one that matters most — asking past the end is a request per scroll for an answer the
 * provider already gave.
 */
export function mayLoad(inFlight: boolean, loading: boolean, hasMore: boolean): boolean {
  return !inFlight && !loading && hasMore;
}

/**
 * Whether a provider's "there is more" is worth acting on.
 *
 * **An empty page that promises more is a contradiction, and acting on the promise is an infinite
 * loop**: the reader reaches the end, the grid asks again, and the provider answers empty for ever.
 * The empty page wins — there is nothing to append and nothing to draw, so the list has ended
 * whatever the flag says. A provider that answers nothing once and then answers properly on the next
 * attempt is a provider that lost a page, and the reader can reach it by changing a filter; a grid
 * that spins is not recoverable at all.
 */
export function stillHasMore(pageSize: number, hasMore: boolean): boolean {
  return hasMore && pageSize > 0;
}
