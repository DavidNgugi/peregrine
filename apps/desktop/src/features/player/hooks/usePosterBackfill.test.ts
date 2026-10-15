import { describe, expect, it } from "vitest";

import { nextToCapture } from "./usePosterBackfill.js";

describe("nextToCapture", () => {
  it("answers the first title that has not been tried, in the order the read gave", () => {
    // The order is the read's, which is newest first: a reader scrolling a fresh library sees the posters
    // arrive where they are looking rather than from the other end.
    expect(nextToCapture([9, 4, 7], new Set())).toBe(9);
    expect(nextToCapture([9, 4, 7], new Set([9]))).toBe(4);
    expect(nextToCapture([9, 4, 7], new Set([9, 4]))).toBe(7);
  });

  it("answers nothing when every title has been tried", () => {
    // The condition that ends the loop. A version that answered the first title again would capture the
    // same poster for ever.
    expect(nextToCapture([9, 4], new Set([9, 4]))).toBeNull();
    expect(nextToCapture([], new Set())).toBeNull();
  });

  it("captures a title once however often the read names it", () => {
    // A page that lists one title twice — two items of a film, a cursor read twice — must not be two
    // captures: the second would overwrite the first with a frame from the same file.
    expect(nextToCapture([9, 9], new Set())).toBe(9);
    expect(nextToCapture([9, 9], new Set([9]))).toBeNull();
  });

  it("skips a title a previous run refused rather than retrying it for ever", () => {
    // A refusal is usually a property of the file — a container the platform will not seek, a codec it
    // will not decode — so retrying is a decode per re-render for an answer that will not change.
    const tried = new Set([1, 2, 3]);
    expect(nextToCapture([1, 2, 3], tried)).toBeNull();
    // A title the read had not named before is still offered, so a later scan is not blocked by an
    // earlier refusal.
    expect(nextToCapture([1, 2, 3, 4], tried)).toBe(4);
  });
});
