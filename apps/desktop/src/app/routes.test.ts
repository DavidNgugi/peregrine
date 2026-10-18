import { describe, expect, it } from "vitest";
import { HOME, parseRoute, routePath, ROUTE_FOR_ACTION, type Route } from "./routes.js";

const ROUTES: readonly Route[] = [
  { kind: "discover" },
  { kind: "discover", titleKind: "movie" },
  { kind: "discover", titleKind: "show" },
  { kind: "browse" },
  { kind: "browse", titleKind: "movie" },
  { kind: "browse", titleKind: "show" },
  { kind: "library" },
  { kind: "diagnostics" },
  { kind: "settings" },
  { kind: "title", titleKind: "movie", id: 7 },
  { kind: "title", titleKind: "show", id: 12_345 },
  { kind: "person", id: 7 },
  { kind: "player", id: 7 },
  { kind: "player", id: 7, titleKind: "show" },
];

describe("parseRoute", () => {
  it("round-trips every route it declares", () => {
    // The property the module exists for: a route the sidebar can open and a path a link can carry
    // cannot disagree, because one function writes the path and the other reads it.
    for (const route of ROUTES) {
      expect(parseRoute(routePath(route))).toEqual(route);
    }
  });

  it("reads the player's title id, and refuses one it cannot act on", () => {
    // The player is a place rather than a state: a route is what makes Back mean what it says, and an
    // id this build cannot act on is a path it does not open — the same rule the title route follows,
    // so the screen never opens and then fails.
    expect(parseRoute("/player/7")).toEqual({ kind: "player", id: 7 });
    expect(routePath({ kind: "player", id: 7 })).toBe("/player/7");

    for (const path of [
      "/player/",
      "/player/0",
      "/player/-1",
      "/player/7.5",
      "/player/abc",
      "/player",
    ]) {
      expect(parseRoute(path)).toBeNull();
    }
  });

  it("carries a player's episode only as a complete pair", () => {
    const withEpisode = { kind: "player" as const, id: 7, season: 1, episode: 2 };
    expect(parseRoute(routePath(withEpisode))).toEqual(withEpisode);
    expect(routePath(withEpisode)).toBe("/player/7?season=1&episode=2");
    // A season without an episode names nothing to open, so the route falls back to the title's own
    // file rather than carrying half a pair the player cannot resolve.
    expect(parseRoute("/player/7?season=1")).toEqual({ kind: "player", id: 7 });
  });

  it("reads only supported Discover type filters", () => {
    expect(parseRoute("/?type=show#top")).toEqual({ kind: "discover", titleKind: "show" });
    expect(parseRoute("/?type=movie")).toEqual({ kind: "discover", titleKind: "movie" });
    expect(parseRoute("/?type=book")).toEqual(HOME);
    expect(parseRoute("/?type=")).toEqual(HOME);
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

  it("reads a title's kind and id out of its path", () => {
    // The map's `/title/:kind/:id`. The kind is in the address so the screen can name itself before
    // its read answers, and the id is checked rather than trusted: a path is untrusted input.
    expect(parseRoute("/title/movie/7")).toEqual({ kind: "title", titleKind: "movie", id: 7 });
    expect(parseRoute("/title/show/12345/")).toEqual({
      kind: "title",
      titleKind: "show",
      id: 12_345,
    });
  });

  it("reads a browse category out of its path, and refuses one it does not have", () => {
    // The map's `/browse/:type?`. The optional half is the *default* kind rather than All, because a
    // provider's queried list is one kind — there is no endpoint that answers a mixed page — so a
    // bare `/browse` is a real route and `book` is not a category this build can draw.
    expect(parseRoute("/browse")).toEqual({ kind: "browse" });
    expect(parseRoute("/browse/movie")).toEqual({ kind: "browse", titleKind: "movie" });
    expect(parseRoute("/browse/show/")).toEqual({ kind: "browse", titleKind: "show" });
    for (const bad of ["/browse/book", "/browse/movie/7", "/browse/movie/extra"]) {
      expect(parseRoute(bad), bad).toBeNull();
    }
  });

  it("refuses a title path whose kind or id this build cannot use", () => {
    for (const bad of [
      "/title/book/7",
      "/title/movie/0",
      "/title/movie/-1",
      "/title/movie/1.5",
      "/title/movie",
      "/title/movie/7/extra",
      "/title/movie/not-a-number",
      "/title/movie/9007199254740993",
    ]) {
      expect(parseRoute(bad), bad).toBeNull();
    }
  });

  it("answers nothing for a path this build has no screen for", () => {
    // The map's other rows are not guessed at: `/search` is a real route in ARCHITECTURE and no
    // screen here can open it, and answering with the home route would make a link to it look like a
    // link home.
    // `/browse` used to be in this list, for the reason the comment in this test used to give; it has
    // a screen now, so it moved to the cases above — the same move `/title/movie/1` made.
    for (const unknown of ["/search", "/library/../settings", "", "/LIBRARY"]) {
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
