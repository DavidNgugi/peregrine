import { describe, expect, it } from "vitest";

import { byRecentlyAdded } from "./library-order.js";

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
