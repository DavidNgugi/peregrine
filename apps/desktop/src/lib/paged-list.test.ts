import type { RailItemDto } from "@peregrine/ipc-client";
import { describe, expect, it } from "vitest";
import { mayLoad, mergePage, stillHasMore } from "./paged-list.js";

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

describe("mergePage", () => {
  it("replaces what is drawn when the page is the first of a query", () => {
    // The rule a filter change depends on: the new answer is the answer, not an addition to the old
    // one. A grid that appended here would show the previous filter's titles above the new ones.
    expect(mergePage([item(1), item(2)], [item(3)], true)).toEqual([item(3)]);
  });

  it("appends a later page in the provider's order", () => {
    expect(mergePage([item(1), item(2)], [item(3), item(4)], false)).toEqual([
      item(1),
      item(2),
      item(3),
      item(4),
    ]);
  });

  it("keeps one tile for a title two pages both hold", () => {
    // The real case, and not a hypothetical one: `sort_by=popularity.desc` is ordered by a number
    // that moves while a reader scrolls, so a title can slide from page one to page two between the
    // two requests. The grid keys its tiles by the catalogue row, so the second copy is a duplicate
    // React key and two tiles that select as one.
    const merged = mergePage([item(1), item(2)], [item(2), item(3)], false);

    expect(merged.map((one) => one.titleId)).toEqual([1, 2, 3]);
  });

  it("keeps the first copy of a repeat, which is the one the provider ranked first", () => {
    const merged = mergePage([item(1, "First seen")], [item(1, "Second seen")], false);

    expect(merged).toHaveLength(1);
    expect(merged[0]?.name).toBe("First seen");
  });

  it("repeats inside one page as one tile", () => {
    // A provider that lists a title twice in one answer is malformed, and the screen's answer to
    // malformed input is still one tile rather than a broken grid.
    expect(mergePage([], [item(1), item(1)], false)).toHaveLength(1);
  });

  it("leaves an empty answer empty rather than carrying the previous one forward", () => {
    // §4's empty case at the seam: the last page of a query can hold nothing, and what is on screen
    // then is what the pages before it held — not an error and not a stale list.
    expect(mergePage([item(1)], [], false)).toEqual([item(1)]);
    expect(mergePage([item(1)], [], true)).toEqual([]);
  });
});

describe("mayLoad", () => {
  it("asks only when there is a next page and nothing is in the air", () => {
    expect(mayLoad(false, false, true)).toBe(true);
    // The grid calls this on every render near the end, so the in-flight flag is what stops a slow
    // page from being requested once per render.
    expect(mayLoad(true, false, true)).toBe(false);
    expect(mayLoad(false, true, true)).toBe(false);
    // The provider already said there is no next page: asking again is a request per scroll for an
    // answer it gave.
    expect(mayLoad(false, false, false)).toBe(false);
  });
});

describe("stillHasMore", () => {
  it("believes a page that carried something", () => {
    expect(stillHasMore(20, true)).toBe(true);
    // The honest end of a list: a short final page with nothing after it.
    expect(stillHasMore(3, false)).toBe(false);
  });

  it("does not believe a page that carried nothing", () => {
    // The contradiction that makes a grid spin for ever: the reader reaches the end, the grid asks
    // again, and the provider answers empty while still promising more. The empty page wins.
    expect(stillHasMore(0, true)).toBe(false);
    expect(stillHasMore(0, false)).toBe(false);
  });
});
