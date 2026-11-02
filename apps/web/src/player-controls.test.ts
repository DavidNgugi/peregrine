import { describe, expect, it } from "vitest";

import {
  SCRUB_HOLD_GAP_MS,
  inSessionSeconds,
  nextScrubRun,
  playerKey,
  progressPercent,
  scrubStepMs,
  scrubTarget,
} from "./player-controls.js";

describe("playerKey", () => {
  it("reads the D-pad by name and by the code a Tizen build sends instead", () => {
    expect(playerKey("Enter", 13)).toBe("ok");
    expect(playerKey("ArrowLeft", 37)).toBe("left");
    expect(playerKey("Unidentified", 39)).toBe("right");
    expect(playerKey("", 38)).toBe("up");
    expect(playerKey("", 40)).toBe("down");
    expect(playerKey("", 13)).toBe("ok");
  });

  it("ignores every other key, including Back and the transport keys", () => {
    expect(playerKey("", 10_009)).toBeNull();
    expect(playerKey("MediaPlayPause", 10_252)).toBeNull();
    expect(playerKey("a", 65)).toBeNull();
  });
});

describe("nextScrubRun", () => {
  it("starts a run at the first press", () => {
    expect(nextScrubRun(null, 1, 1000)).toEqual({ direction: 1, count: 0, atMs: 1000 });
  });

  it("counts a held key and resets on a pause at the hold gap boundary", () => {
    const first = nextScrubRun(null, 1, 0);
    const held = nextScrubRun(first, 1, SCRUB_HOLD_GAP_MS);
    expect(held.count).toBe(1);
    expect(nextScrubRun(held, 1, held.atMs + SCRUB_HOLD_GAP_MS + 1).count).toBe(0);
  });

  it("restarts when the direction flips or the clock goes backwards", () => {
    const run = { direction: 1 as const, count: 9, atMs: 500 };
    expect(nextScrubRun(run, -1, 600)).toEqual({ direction: -1, count: 0, atMs: 600 });
    expect(nextScrubRun(run, 1, 400).count).toBe(0);
  });
});

describe("scrubStepMs", () => {
  it("keeps a tap precise and accelerates a hold at each threshold", () => {
    expect(scrubStepMs(0)).toBe(10_000);
    expect(scrubStepMs(5)).toBe(10_000);
    expect(scrubStepMs(6)).toBe(30_000);
    expect(scrubStepMs(15)).toBe(30_000);
    expect(scrubStepMs(16)).toBe(60_000);
    expect(scrubStepMs(29)).toBe(60_000);
    expect(scrubStepMs(30)).toBe(120_000);
  });

  it("scales from the fast-forward key's larger base", () => {
    expect(scrubStepMs(0, 30_000)).toBe(30_000);
    expect(scrubStepMs(30, 30_000)).toBe(360_000);
  });
});

describe("scrubTarget", () => {
  it("never goes below zero", () => {
    expect(scrubTarget(4000, -10_000, 60_000)).toBe(0);
  });

  it("stops a second before the end so a scrub does not land on the ending", () => {
    expect(scrubTarget(55_000, 10_000, 60_000)).toBe(59_000);
    expect(scrubTarget(0, 10_000, 500)).toBe(0);
  });

  it("is open-ended when the length is unknown", () => {
    expect(scrubTarget(5000, 10_000, null)).toBe(15_000);
  });
});

describe("inSessionSeconds", () => {
  it("seeks a direct file anywhere", () => {
    expect(inSessionSeconds("direct", 0, 90_000, null)).toBe(90);
    expect(inSessionSeconds("direct", 0, 0, null)).toBe(0);
  });

  it("seeks a transcode only inside what the encoder has written", () => {
    // Started at 10 minutes, 120 seconds encoded so far.
    expect(inSessionSeconds("hls", 600_000, 660_000, 120)).toBe(60);
    expect(inSessionSeconds("hls", 600_000, 600_000, 120)).toBe(0);
    expect(inSessionSeconds("hls", 600_000, 719_000, 120)).toBe(119);
    expect(inSessionSeconds("hls", 600_000, 719_001, 120)).toBeNull();
    expect(inSessionSeconds("hls", 600_000, 599_999, 120)).toBeNull();
  });

  it("restarts when the encoder's edge is unknown or the target is not a position", () => {
    expect(inSessionSeconds("hls", 0, 10_000, null)).toBeNull();
    expect(inSessionSeconds("hls", 0, 10_000, Number.NaN)).toBeNull();
    expect(inSessionSeconds("direct", 0, -1, null)).toBeNull();
    expect(inSessionSeconds("direct", 0, Number.POSITIVE_INFINITY, null)).toBeNull();
  });
});

describe("progressPercent", () => {
  it("is zero without a length and clamps at both ends", () => {
    expect(progressPercent(5000, null)).toBe(0);
    expect(progressPercent(5000, 0)).toBe(0);
    expect(progressPercent(-5, 100)).toBe(0);
    expect(progressPercent(50, 100)).toBe(50);
    expect(progressPercent(500, 100)).toBe(100);
  });
});
