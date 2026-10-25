import { describe, expect, it } from "vitest";

import { parseRoute, routePath } from "./routing.js";

describe("routing", () => {
  it("round-trips every screen", () => {
    const routes = [
      { kind: "home" },
      { kind: "browse" },
      { kind: "myList" },
      { kind: "downloads" },
      { kind: "library" },
      { kind: "integrations" },
      { kind: "title", id: 16 },
      { kind: "watch", id: 16 },
    ] as const;
    for (const route of routes) {
      expect(parseRoute(routePath(route))).toEqual(route);
    }
  });

  it("opens the read-only integrations screen from its own path", () => {
    expect(parseRoute("/integrations")).toEqual({ kind: "integrations" });
    expect(parseRoute("/integrations/")).toEqual({ kind: "integrations" });
  });

  it("treats an unknown path as home", () => {
    expect(parseRoute("/not-a-screen")).toEqual({ kind: "home" });
    expect(parseRoute("/title/not-a-number")).toEqual({ kind: "home" });
  });
});
