import { describe, expect, it } from "vitest";

import { downloadLabel, downloadTone } from "./download-state.js";

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
