import { describe, expect, it } from "vitest";

import { recallRoute, rememberRoute } from "./route-state.js";

describe("route memory", () => {
  it("round-trips the scroll and focus a screen was left at", () => {
    rememberRoute("/browse", { scrollY: 420, focusKey: "tile:16" });
    expect(recallRoute("/browse")).toEqual({ scrollY: 420, focusKey: "tile:16" });
  });

  it("does not pretend an unvisited screen has a place to restore", () => {
    expect(recallRoute("/never-visited")).toBeNull();
  });
});
