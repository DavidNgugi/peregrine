import { describe, expect, it } from "vitest";

import { FRAME_SEEK_MS, frameSeekMs } from "./frame-capture.js";

describe("frameSeekMs", () => {
  it("takes a poster from a tenth of a feature, not from its opening titles", () => {
    // A two-hour film: twelve minutes in, which is past the logos and the credits and into the film.
    expect(frameSeekMs(7_200_000)).toBe(720_000);
  });

  it("never asks a short clip for a frame past its own end", () => {
    // A thirty-second trailer. A tenth of it is three seconds, which is inside it; the assertion that
    // matters is the upper bound, because `seeked` does not fire for a time beyond the duration and a
    // capture that asked for one would wait for an event that never arrives.
    expect(frameSeekMs(30_000)).toBeLessThan(30_000);
    expect(frameSeekMs(5_000)).toBeLessThan(5_000);
    expect(frameSeekMs(1_200)).toBeLessThan(1_200);
    expect(frameSeekMs(1_200)).toBeGreaterThanOrEqual(0);
  });

  it("keeps the floor below five minutes and the tenth above it", () => {
    // **The boundary is five minutes, because the floor is thirty seconds and a tenth of five minutes is
    // also thirty.** Below it the floor decides, which is what stops a short clip's poster being the very
    // first frames; above it the tenth decides, which is what stops a feature's poster being a tenth of the
    // way into it.
    expect(frameSeekMs(60_000)).toBe(FRAME_SEEK_MS);
    expect(frameSeekMs(240_000)).toBe(FRAME_SEEK_MS + 0);
    expect(frameSeekMs(300_000)).toBe(FRAME_SEEK_MS);
    expect(frameSeekMs(600_000)).toBe(60_000);
    expect(frameSeekMs(1_000_000)).toBe(100_000);
  });

  it("answers zero rather than a nonsense time for a duration it cannot use", () => {
    // A seek to `NaN` is a seek that never completes, and the element's own first frame is a better answer
    // than a capture that hangs until its timeout.
    for (const duration of [0, -1, Number.NaN, Number.POSITIVE_INFINITY, Number.NEGATIVE_INFINITY]) {
      expect(frameSeekMs(duration)).toBe(0);
    }
  });

  it("never exceeds the duration, at any boundary", () => {
    // The property rather than the examples: whatever the length, the frame asked for is inside it.
    for (const duration of [1, 999, 1_000, 1_001, 5_000, 299_999, 300_000, 300_001, 3_600_000]) {
      const seek = frameSeekMs(duration);
      expect(seek, `${String(duration)}ms`).toBeLessThanOrEqual(duration);
      expect(seek).toBeGreaterThanOrEqual(0);
    }
  });
});
