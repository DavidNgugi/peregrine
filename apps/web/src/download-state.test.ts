import { describe, expect, it } from "vitest";

import { cardMarks, downloadLabel, downloadTone } from "./download-state.js";

describe("downloadTone", () => {
  it("reads every lifecycle state as one of the five control answers", () => {
    const cases: Array<[string | null, string]> = [
      [null, "none"],
      ["", "none"],
      ["removed", "none"],
      ["queued", "queued"],
      ["inspecting", "queued"],
      ["metadata", "queued"],
      ["paused", "queued"],
      ["downloading", "active"],
      ["extracting", "active"],
      ["repairing", "active"],
      ["importing", "active"],
      ["seeding", "done"],
      ["completed", "done"],
      ["failed", "failed"],
      ["blocked", "failed"],
    ];
    for (const [state, tone] of cases) {
      expect(downloadTone(state), String(state)).toBe(tone);
    }
  });

  it("treats an unknown state as queued rather than as a failure", () => {
    expect(downloadTone("something-new")).toBe("queued");
  });
});

describe("downloadLabel", () => {
  it("names the action a button offers, not the core's internal word", () => {
    expect(downloadLabel(null)).toBe("Download");
    expect(downloadLabel("queued")).toBe("Queued");
    expect(downloadLabel("downloading")).toBe("Downloading");
    expect(downloadLabel("completed")).toBe("Downloaded");
    expect(downloadLabel("failed")).toBe("Try again");
  });
});

describe("cardMarks", () => {
  const row = (titleId: number, state: string, progressPermille = 0) => ({
    titleId,
    state,
    progressPermille,
  });

  it("gives a running transfer its floored percent and a queued one none", () => {
    const marks = cardMarks([row(1, "downloading", 999), row(2, "queued")]);
    expect(marks.get(1)).toEqual({ tone: "active", percent: 99 });
    expect(marks.get(2)).toEqual({ tone: "active", percent: null });
  });

  it("keeps the most urgent job when a title has several", () => {
    const marks = cardMarks([row(1, "completed"), row(1, "failed"), row(1, "downloading", 400)]);
    expect(marks.get(1)).toEqual({ tone: "active", percent: 40 });
    expect(cardMarks([row(2, "completed"), row(2, "failed")]).get(2)?.tone).toBe("failed");
  });

  it("says nothing for a removed job", () => {
    expect(cardMarks([row(3, "removed")]).has(3)).toBe(false);
  });
});
