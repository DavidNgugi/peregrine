import { describe, expect, it } from "vitest";
import { formatTimecode, progressFraction } from "./index.js";

describe("formatTimecode", () => {
  it("formats under an hour as m:ss", () => {
    expect(formatTimecode(0)).toBe("0:00");
    expect(formatTimecode(1_000)).toBe("0:01");
    expect(formatTimecode(59_999)).toBe("0:59");
    expect(formatTimecode(60_000)).toBe("1:00");
    expect(formatTimecode(3_599_999)).toBe("59:59");
  });

  it("adds the hour field from the first hour", () => {
    expect(formatTimecode(3_600_000)).toBe("1:00:00");
    expect(formatTimecode(3_661_000)).toBe("1:01:01");
    expect(formatTimecode(359_999_999)).toBe("99:59:59");
  });

  it("never prints a non-finite or negative timecode", () => {
    expect(formatTimecode(-1)).toBe("0:00");
    expect(formatTimecode(Number.NaN)).toBe("0:00");
    expect(formatTimecode(Number.POSITIVE_INFINITY)).toBe("0:00");
  });
});

describe("progressFraction", () => {
  it("clamps to the unit interval", () => {
    expect(progressFraction(0, 1_000)).toBe(0);
    expect(progressFraction(500, 1_000)).toBe(0.5);
    expect(progressFraction(1_000, 1_000)).toBe(1);
    expect(progressFraction(2_000, 1_000)).toBe(1);
    expect(progressFraction(-10, 1_000)).toBe(0);
  });

  it("reports zero for an unknown or zero duration", () => {
    expect(progressFraction(500, 0)).toBe(0);
    expect(progressFraction(500, -1)).toBe(0);
    expect(progressFraction(500, Number.NaN)).toBe(0);
    expect(progressFraction(Number.NaN, 1_000)).toBe(0);
    expect(progressFraction(Number.POSITIVE_INFINITY, 1_000)).toBe(0);
  });
});
