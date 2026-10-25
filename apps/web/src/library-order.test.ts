import { describe, expect, it } from "vitest";

import { byRecentlyAdded, matchesKind } from "./library-order.js";

describe("byRecentlyAdded", () => {
  it("puts the newest addition first and keeps the id order for a tie", () => {
    const rows = [
      { id: 1, addedAt: 10 },
      { id: 3, addedAt: 30 },
      { id: 2, addedAt: 20 },
    ];
    expect(byRecentlyAdded(rows).map((row) => row.id)).toEqual([3, 2, 1]);
    expect(
      byRecentlyAdded([
        { id: 2, addedAt: 10 },
        { id: 1, addedAt: 10 },
      ]).map((row) => row.id),
    ).toEqual([2, 1]);
  });

  it("does not mutate the read", () => {
    const rows = [
      { id: 1, addedAt: 10 },
      { id: 2, addedAt: 20 },
    ];
    byRecentlyAdded(rows);
    expect(rows.map((row) => row.id)).toEqual([1, 2]);
  });
});

describe("matchesKind", () => {
  it("accepts both spellings the two library reads use", () => {
    // The LAN route answers `movie`/`show`; the desktop DTO answers `movies`/`shows`. The tab
    // filtered on one spelling and hid every row from the other.
    expect(matchesKind("movie", "movie")).toBe(true);
    expect(matchesKind("movies", "movie")).toBe(true);
    expect(matchesKind("show", "show")).toBe(true);
    expect(matchesKind("shows", "show")).toBe(true);
    expect(matchesKind("movie", "show")).toBe(false);
    expect(matchesKind("shows", "all")).toBe(true);
  });
});
