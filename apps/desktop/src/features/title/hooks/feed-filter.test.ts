import type { RailItemDto } from "@peregrine/ipc-client";
import { describe, expect, it } from "vitest";
import { mergePage } from "../../../lib/paged-list.js";
import { withoutTitle } from "./feed-filter.js";

/**
 * Keeping the opened title out of its own feed.
 *
 * The hook itself cannot be driven here: `vitest.config.ts` runs in a Node environment with no DOM,
 * and `renderToStaticMarkup` never runs an effect, so `useTitleFeed` is not observable. What the
 * hook does after a page arrives is `withoutTitle(mergePage(...))`, and that is what these tests
 * hold: the recommendation head, and every catalogue page appended after it.
 */

function item(titleId: number, name = `Title ${String(titleId)}`): RailItemDto {
  return {
    titleId,
    kind: "movie",
    name,
    year: null,
    poster: null,
    backdrop: null,
    playable: false,
    rating: null,
    overview: null,
    votes: null,
  };
}

describe("withoutTitle", () => {
  it("drops the opened title from the recommendation head", () => {
    // The core's `similar` read already excludes the opened local title and a Rust test holds that,
    // but the seed is filtered here too so no caller can hand the feed a head that names the page it
    // is on.
    const head = [item(4), item(7, "Opened"), item(5)];

    expect(withoutTitle(head, 7).map((one) => one.titleId)).toEqual([4, 5]);
  });

  it("keeps the opened title out when a shared genre's page brings it back", () => {
    // **The regression this file exists for.** A genre browse page is the same for every title in
    // the genre, so it knows nothing about which page asked for it: the page's own title can arrive
    // on page one of a genre after the core had already filtered it from the head. The merge applies
    // the overlap rule, and the filter after it is what stops the feed recommending the page it is
    // on.
    const seed = [item(4), item(5)];
    const genrePage = [item(7, "Opened"), item(6)];

    const merged = mergePage(seed, genrePage, false);
    expect(withoutTitle(merged, 7).map((one) => one.titleId)).toEqual([4, 5, 6]);
  });

  it("filters by local title id rather than by name", () => {
    // A duplicate provider entry, or a name the matcher merged, can make two local titles read the
    // same. Only the one whose page is open is the page itself; dropping every title with its name
    // would hide a real suggestion instead.
    const items = [item(7, "The Matrix"), item(8, "The Matrix")];

    expect(withoutTitle(items, 7).map((one) => one.titleId)).toEqual([8]);
  });

  it("filters nothing when no title route is open", () => {
    // `null` is every screen but a title page. Returning the list unchanged is the truthful answer:
    // there is no page to keep out of its own feed, so there is nothing to copy or drop.
    const items = [item(7), item(4)];

    expect(withoutTitle(items, null)).toBe(items);
  });

  it("leaves an empty feed empty, and a feed without the opened title untouched", () => {
    expect(withoutTitle([], 7)).toEqual([]);

    const items = [item(4), item(5)];
    expect(withoutTitle(items, 7)).toEqual(items);
  });
});
