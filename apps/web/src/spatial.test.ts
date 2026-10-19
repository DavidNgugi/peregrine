import { describe, expect, it } from "vitest";

import { nextIndex, type Rect } from "./spatial.js";

/** A 3x2 grid of 100x150 tiles with 10px gaps. */
function grid(): Rect[] {
  const rects: Rect[] = [];
  for (let row = 0; row < 2; row += 1) {
    for (let column = 0; column < 3; column += 1) {
      rects.push({ x: column * 110, y: row * 160, width: 100, height: 150 });
    }
  }
  return rects;
}

describe("nextIndex", () => {
  it("moves along a row and down a column", () => {
    const rects = grid();
    expect(nextIndex(rects, 0, "right")).toBe(1);
    expect(nextIndex(rects, 1, "right")).toBe(2);
    expect(nextIndex(rects, 0, "down")).toBe(3);
    expect(nextIndex(rects, 3, "down")).toBe(3);
  });

  it("stops at the edge instead of wrapping", () => {
    const rects = grid();
    expect(nextIndex(rects, 2, "right")).toBe(2);
    expect(nextIndex(rects, 0, "left")).toBe(0);
    expect(nextIndex(rects, 0, "up")).toBe(0);
    expect(nextIndex(rects, 5, "down")).toBe(5);
  });

  it("keeps focus when the grid is empty or the index is not one", () => {
    expect(nextIndex([], 0, "right")).toBe(0);
    expect(nextIndex(grid(), 99, "right")).toBe(99);
  });

  it("prefers the same row over a diagonal tile in the next one", () => {
    const rects: Rect[] = [
      { x: 0, y: 0, width: 100, height: 150 },
      { x: 110, y: 0, width: 100, height: 150 },
      { x: 90, y: 160, width: 100, height: 150 },
    ];
    expect(nextIndex(rects, 0, "right")).toBe(1);
  });
});
