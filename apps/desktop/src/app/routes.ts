/**
 * The application's route space, as types and as paths.
 *
 * `docs/ARCHITECTURE.md` §6 is the routing map, and this module is the part of it this build has the
 * screens for: Discover, Browse, Library, a title, the player, Diagnostics, and Settings. A route for a screen that does
 * not exist yet would be a path nothing can open, so the map's other rows land with their screens, the
 * same rule `screens.ts` states for the sidebar.
 *
 * **Why a table rather than the router package.** `PG-E7-02` names typed routes and cache-first
 * loaders; the loaders are what the router package would carry, and the screens that exist read
 * through hooks that render immediately rather than blocking. What is left is the route space
 * itself, which is five paths, two optional parameters, and three consumers that must agree on it:
 * the sidebar, the shortcut dispatcher, and the link that arrives from outside the application. A
 * package that resolves paths would be a dependency this half does not need — and the day the
 * loaders land, the routes are already typed, which is the half that outlives either choice.
 *
 * Paths are parsed and formatted by *these* functions, so a route the sidebar can open and a path a
 * link can carry cannot disagree: `parseRoute(routePath(route))` is the route, and its test says so.
 */

/** A place in the application. */
export type Route =
  | { readonly kind: "discover"; readonly titleKind?: TitleKind }
  /**
   * The filtered grid: every avenue into the catalogue that a rail is not.
   *
   * Its `titleKind` is optional for the same reason Discover's is — a bare `/browse` is **All**, and
   * "All" is a choice rather than the absence of one — but it travels in the *path* rather than a
   * query string, because `ARCHITECTURE` §6's map spells the row `/browse/:type?` and because a
   * category is a place a person links to: `/browse/movie` says what it is with no query parsing.
   */
  | { readonly kind: "browse"; readonly titleKind?: TitleKind }
  | { readonly kind: "library" }
  | { readonly kind: "downloads" }
  | { readonly kind: "diagnostics" }
  | { readonly kind: "settings" }
  /**
   * One title's own screen.
   *
   * The kind travels in the path because `ARCHITECTURE` §6's map says it does, and because it is
   * what a screen can draw before its read answers: `Film` or `Series` is known from the address
   * alone, so the page names itself immediately rather than waiting (PG-E7-02's acceptance).
   */
  | { readonly kind: "title"; readonly titleKind: TitleKind; readonly id: number }
  /**
   * The player: one title's file, playing.
   *
   * `ARCHITECTURE` §6 spells this row `/player/:sessionId`, and this build's parameter is the *title*
   * — because it has no session registry yet. `peregrine-app` holds at most one session and a session
   * is opened for a title, so the title is what addresses it; `PG-E16-02`'s `PlayerBackend` is what
   * makes a session an addressable thing in its own right, and the path is where that will show.
   */
  | { readonly kind: "player"; readonly id: number };

/** Which of the two a title is, as a path spells it. */
export type TitleKind = "movie" | "show";

/** What a route is called when it needs naming. */
export type RouteKind = Route["kind"];

/** The routes that are one fixed path, which are the ones a table can hold. */
type StaticRouteKind = Exclude<RouteKind, "title" | "player">;

/**
 * The path each route lives at, taken from `docs/ARCHITECTURE.md` §6.
 *
 * The map spells Diagnostics `/diagnostics` while the feature folder is `jobs`: the screen shows the
 * job journal, and the architecture's name for the screen is the one a link uses.
 */
const PATHS: Readonly<Record<StaticRouteKind, string>> = {
  discover: "/",
  browse: "/browse",
  library: "/library",
  downloads: "/downloads",
  diagnostics: "/diagnostics",
  settings: "/settings",
};

/** The route the application opens at when nothing else says otherwise. */
export const HOME: Route = { kind: "discover" };

/**
 * The route a path names, or `null` for a path this build has no screen for.
 *
 * `null` rather than a fallback because the two callers need different answers: a link that names an
 * unknown route is refused by name, and a window that opens at one shows the home screen. Returning
 * the home route here would make those the same case.
 *
 * A trailing slash is accepted for the same path, and anything after the route — a query string, a
 * fragment — is ignored except Discover’s `type` filter, because a path this build knows is still that path when it carries state it
 * does not read yet. Case is not folded: paths are compared as written, which is what the map's
 * lowercase rows mean.
 */
export function parseRoute(path: string): Route | null {
  const withoutTail = path.split(/[?#]/u, 1)[0] ?? "";
  const normalized = withoutTail.length > 1 ? withoutTail.replace(/\/+$/u, "") : withoutTail;

  for (const [kind, routePath] of Object.entries(PATHS) as [StaticRouteKind, string][]) {
    if (routePath === normalized) {
      if (kind === "discover") {
        const filter = new URLSearchParams(path.split("?")[1]?.split("#")[0]).get("type");
        if (filter === "movie" || filter === "show") {
          return { kind, titleKind: filter };
        }
      }
      return { kind };
    }
  }

  // `/browse/:type?`, which is the map's row for the filtered grid. `/browse` alone is the exact
  // path the table above matched; this is the one with a kind, checked against the two this build has
  // rather than carried through as an opaque string.
  const parts = normalized.split("/");
  if (parts.length === 3 && parts[0] === "" && parts[1] === "browse") {
    const kind = parts[2];
    if (kind === "movie" || kind === "show") {
      return { kind: "browse", titleKind: kind };
    }
    return null;
  }

  // `/title/:kind/:id`, which is the map's row for a title's own screen. The kind is checked against
  // the two the core names rather than carried through as an opaque string: a path claiming a kind
  // this build does not have is a path this build does not open.
  if (parts.length === 4 && parts[0] === "" && parts[1] === "title") {
    const kind = parts[2];
    const id = Number(parts[3]);
    if ((kind === "movie" || kind === "show") && Number.isSafeInteger(id) && id > 0) {
      return { kind: "title", titleKind: kind, id };
    }
  }

  // `/player/:id`, which is the map's row for the player. An id this build cannot act on is a path
  // this build does not open, for the reason the title route gives: a route that could not resolve
  // would be a screen that opened and then failed.
  if (parts.length === 3 && parts[0] === "" && parts[1] === "player") {
    const id = Number(parts[2]);
    if (Number.isSafeInteger(id) && id > 0) {
      return { kind: "player", id };
    }
  }
  return null;
}

/** The path a route lives at. */
export function routePath(route: Route): string {
  if (route.kind === "discover" && route.titleKind !== undefined) {
    return `/?type=${route.titleKind}`;
  }
  if (route.kind === "browse" && route.titleKind !== undefined) {
    return `/browse/${route.titleKind}`;
  }
  if (route.kind === "title") {
    return `/title/${route.titleKind}/${String(route.id)}`;
  }
  if (route.kind === "player") {
    return `/player/${String(route.id)}`;
  }
  return PATHS[route.kind];
}

/** The route a shortcut action opens, or `null` for an action this shell has no screen for. */
export const ROUTE_FOR_ACTION: Readonly<Record<string, Route>> = {
  "navigation.discover": { kind: "discover" },
  "navigation.jobs": { kind: "diagnostics" },
  "navigation.settings": { kind: "settings" },
};
