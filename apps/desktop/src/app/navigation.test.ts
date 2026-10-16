import { describe, expect, it } from "vitest";
import {
  back,
  canGoBack,
  MAX_HISTORY,
  navigate,
  sameRoute,
  startHistory,
} from "./navigation.js";
import { HOME, type Route } from "./routes.js";

const settings: Route = { kind: "settings" };
const library: Route = { kind: "library" };
const title: Route = { kind: "title", titleKind: "movie", id: 7 };
const browse: Route = { kind: "browse", titleKind: "show" };

describe("navigation history", () => {
  it("has nowhere to go back to at the start", () => {
    const history = startHistory(HOME);
    expect(canGoBack(history)).toBe(false);
    expect(back(history)).toBe(history);
  });

  it("remembers each place after a visit", () => {
    const history = navigate(navigate(startHistory(HOME), library), settings);
    expect(canGoBack(history)).toBe(true);
    expect(back(history).current).toEqual(library);
    expect(back(back(history)).current).toEqual(HOME);
    expect(canGoBack(back(back(history)))).toBe(false);
  });

  it("treats a move to where the user already is as no move at all", () => {
    // The current sidebar row is a button a person can press; pressing it must not make Back
    // appear to do nothing.
    const history = navigate(startHistory(HOME), { kind: "discover" });
    expect(history).toEqual(startHistory(HOME));
  });

  it("compares routes by the path they live at", () => {
    // Compared through `routePath`, so a route with a kind and one without are different places
    // while two spellings of the same place are not.
    expect(sameRoute({ kind: "browse" }, { kind: "browse" })).toBe(true);
    expect(sameRoute(browse, { kind: "browse", titleKind: "movie" })).toBe(false);
    expect(sameRoute(title, { kind: "title", titleKind: "movie", id: 8 })).toBe(false);
  });

  it("drops the places ahead when a new move follows a back", () => {
    // A history and not a list: after going back, opening somewhere else forgets the old future.
    const three = navigate(navigate(startHistory(HOME), library), settings);
    const backOnce = back(three);
    const elsewhere = navigate(backOnce, title);
    expect(elsewhere.current).toEqual(title);
    expect(back(elsewhere).current).toEqual(library);
  });

  it("keeps at most MAX_HISTORY places", () => {
    // The oldest visit is the one dropped, so Back still walks the recent past.
    let history = startHistory(HOME);
    for (let id = 1; id <= MAX_HISTORY + 20; id += 1) {
      history = navigate(history, { kind: "title", titleKind: "movie", id });
    }
    expect(history.past).toHaveLength(MAX_HISTORY);
    expect(history.past[0]).toEqual({ kind: "title", titleKind: "movie", id: 20 });
    expect(history.current).toEqual({ kind: "title", titleKind: "movie", id: MAX_HISTORY + 20 });
  });

  it("carries a title's own id through the same stack as a screen", () => {
    const history = navigate(startHistory(HOME), title);
    expect(history.current).toEqual(title);
    expect(back(history).current).toEqual(HOME);
  });
});
