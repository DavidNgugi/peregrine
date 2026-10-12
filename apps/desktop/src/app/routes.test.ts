import { describe, expect, it } from "vitest";
import { HOME, parseRoute, routePath, ROUTE_FOR_ACTION, type Route } from "./routes.js";

const ROUTES: readonly Route[] = [
  { kind: "discover" },
  { kind: "library" },
  { kind: "diagnostics" },
  { kind: "settings" },
];

describe("parseRoute", () => {
  it("round-trips every route it declares", () => {
    // The property the module exists for: a route the sidebar can open and a path a link can carry
    // cannot disagree, because one function writes the path and the other reads it.
    for (const route of ROUTES) {
      expect(parseRoute(routePath(route))).toEqual(route);
    }
  });

  it("opens the home screen for the root path", () => {
    expect(parseRoute("/")).toEqual(HOME);
  });

  it("accepts a trailing slash for the same route", () => {
    // A path typed, pasted, or written by a link builder with a trailing separator still names the
    // route it names; refusing it would be a difference nobody could see.
    expect(parseRoute("/library/")).toEqual({ kind: "library" });
    expect(parseRoute("//")).toBeNull();
  });

  it("ignores a query string and a fragment", () => {
    // A path this build knows is still that path when it carries state it does not read yet.
    expect(parseRoute("/library?sort=added")).toEqual({ kind: "library" });
    expect(parseRoute("/settings#media")).toEqual({ kind: "settings" });
  });

  it("answers nothing for a path this build has no screen for", () => {
    // The map's other rows are not guessed at: `/browse` is a real route in ARCHITECTURE and no
    // screen here can open it, and answering with the home route would make a link to it look like
    // a link home.
    for (const unknown of ["/browse", "/title/movie/1", "/library/../settings", "", "/LIBRARY"]) {
      expect(parseRoute(unknown)).toBeNull();
    }
  });
});

describe("ROUTE_FOR_ACTION", () => {
  it("opens only routes the sidebar lists", () => {
    // What PG-E7-01's acceptance is about, at the route layer: a shortcut cannot open a place the
    // navigation does not have, so the two tables are checked against each other rather than trusted.
    const declared = new Set(ROUTES.map(routePath));
    for (const route of Object.values(ROUTE_FOR_ACTION)) {
      expect(declared.has(routePath(route))).toBe(true);
    }
  });
});
