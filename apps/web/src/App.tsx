/**
 * The ten-foot and phone client: pair, home, library, a title's page, and the player (ADR 0028).
 *
 * The shape is Netflix's, reduced to what this core actually holds: a hero over horizontal rails,
 * a title page with episodes for a series and More Like This for anything the provider can name,
 * and a player that writes the same progress row the desktop player writes.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import {
  api,
  ApiError,
  type LibraryRow,
  type PairOffer,
  type PlaybackTicket,
  type Tile,
} from "./api.js";
import { BrowseScreen } from "./Browse.js";
import { describeDevice } from "./device.js";
import { DownloadsScreen } from "./Downloads.js";
import { useAutoFocus, useSpatialFocus } from "./focus.js";
import { HomeScreen } from "./Home.js";
import { byRecentlyAdded } from "./library-order.js";
import {
  BookmarkIcon,
  ChevronLeftIcon,
  CompassIcon,
  DownloadIcon,
  HomeIcon,
  LibraryIcon,
  SearchIcon,
} from "./icons.js";
import { MyListScreen } from "./MyList.js";
import { Player } from "./Player.js";
import { ProfileMenu } from "./ProfileMenu.js";
import { parseRoute, routePath, type Route } from "./routing.js";
import { backTarget, exitTizenApp, isTizen, useTizenBack, useTizenFocusPlate } from "./tizen.js";
import { TitleScreen } from "./Title.js";

type Screen =
  | { readonly name: "loading" }
  | { readonly name: "pair" }
  | { readonly name: "home" }
  | { readonly name: "browse" }
  | { readonly name: "myList" }
  | { readonly name: "downloads" }
  | { readonly name: "library" }
  | { readonly name: "title"; readonly id: number; readonly preview?: Tile }
  | {
      readonly name: "player";
      readonly ticket: PlaybackTicket;
      readonly titleId: number;
      readonly title: string;
    };

export function App() {
  const [screen, setScreen] = useState<Screen>({ name: "loading" });
  const [home, setHome] = useState<Awaited<ReturnType<typeof api.home>> | null>(null);
  const [homeLoading, setHomeLoading] = useState(true);
  const [homeError, setHomeError] = useState<string | null>(null);
  const [rows, setRows] = useState<readonly LibraryRow[]>([]);
  const [libraryError, setLibraryError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [canAcquire, setCanAcquire] = useState(false);
  const screenRef = useRef(screen);
  screenRef.current = screen;

  const loadHome = useCallback(async (): Promise<void> => {
    setHomeLoading(true);
    try {
      const data = await api.home();
      setHome(data);
      setHomeError(null);
    } catch (caught) {
      if (caught instanceof ApiError && caught.status === 401) {
        setScreen({ name: "pair" });
        return;
      }
      setHomeError(text(caught));
    } finally {
      setHomeLoading(false);
    }
  }, []);

  const loadLibrary = useCallback(async (): Promise<void> => {
    try {
      const library = await api.library();
      setRows(byRecentlyAdded(library));
      setLibraryError(null);
      setScreen({ name: "library" });
    } catch (caught) {
      if (caught instanceof ApiError && caught.status === 401) {
        setScreen({ name: "pair" });
        return;
      }
      setLibraryError(text(caught));
      setScreen({ name: "library" });
    }
  }, []);

  const push = useCallback((route: Route): void => {
    const path = routePath(route);
    if (window.location.pathname + window.location.search !== path) {
      window.history.pushState({ fromApp: true }, "", path);
    }
  }, []);

  const showRoute = useCallback(
    (route: Route, preview?: Tile): void => {
      switch (route.kind) {
        case "home":
          // An unknown path is the home screen; give it the address that says so.
          if (window.location.pathname !== "/") {
            window.history.replaceState(null, "", "/");
          }
          setScreen({ name: "home" });
          // Re-read on every return: the watchlist marks on the rails are part of the answer, and a
          // title saved from its page must show its bookmark when Back lands here.
          void loadHome();
          break;
        case "browse":
          setScreen({ name: "browse" });
          break;
        case "myList":
          setScreen({ name: "myList" });
          break;
        case "downloads":
          setScreen({ name: "downloads" });
          break;
        case "library":
          setScreen({ name: "library" });
          void loadLibrary();
          break;
        case "title":
          setScreen(
            preview === undefined
              ? { name: "title", id: route.id }
              : { name: "title", id: route.id, preview },
          );
          break;
        case "watch": {
          const title: Route = { kind: "title", id: route.id };
          window.history.replaceState({ fromApp: false }, "", routePath(title));
          setScreen({ name: "title", id: route.id });
          break;
        }
      }
    },
    [loadHome, loadLibrary],
  );

  // A refresh on any path but the home screen still needs an authenticated read; the library is
  // the smallest one, and it is what `/library` would draw anyway.
  useEffect(() => {
    let cancelled = false;
    const route = parseRoute(window.location.pathname);
    const boot = async (): Promise<void> => {
      setHomeLoading(true);
      try {
        const me = await api.me();
        if (cancelled) return;
        setCanAcquire(me.canAcquire);
        if (route.kind === "home") {
          const data = await api.home();
          if (cancelled) return;
          setHome(data);
          setHomeError(null);
          if (window.location.pathname !== "/") {
            window.history.replaceState(null, "", "/");
          }
          setScreen({ name: "home" });
        } else {
          await api.library();
          if (cancelled) return;
          showRoute(route);
        }
      } catch (caught) {
        if (cancelled) return;
        if (caught instanceof ApiError && caught.status === 401) {
          setScreen({ name: "pair" });
        } else if (route.kind === "home") {
          setHomeError(text(caught));
          setScreen({ name: "home" });
        } else {
          setLibraryError(text(caught));
          showRoute(route);
        }
      } finally {
        if (!cancelled) setHomeLoading(false);
      }
    };
    void boot();
    return () => {
      cancelled = true;
    };
    // The first route is read once; later routes arrive through popstate or an explicit push.
  }, []);

  const goHome = useCallback((): void => {
    push({ kind: "home" });
    setScreen({ name: "home" });
    void loadHome();
  }, [loadHome, push]);

  const goLibrary = useCallback((): void => {
    push({ kind: "library" });
    setScreen({ name: "library" });
    void loadLibrary();
  }, [loadLibrary, push]);

  const goBrowse = useCallback(
    (focusSearch = false): void => {
      push({ kind: "browse" });
      setScreen({ name: "browse" });
      if (focusSearch) {
        window.setTimeout(() => document.getElementById("browse-search")?.focus(), 60);
      }
    },
    [push],
  );

  const goMyList = useCallback((): void => {
    push({ kind: "myList" });
    setScreen({ name: "myList" });
  }, [push]);

  const goDownloads = useCallback((): void => {
    push({ kind: "downloads" });
    setScreen({ name: "downloads" });
  }, [push]);

  const goBack = useCallback((): void => {
    if ((window.history.state as { fromApp?: boolean } | null)?.fromApp === true) {
      window.history.back();
    } else {
      // A directly-opened page has no earlier app entry; replace it with home so Back does not
      // bounce between the same two screens.
      window.history.replaceState(null, "", "/");
      setScreen({ name: "home" });
      void loadHome();
    }
  }, [loadHome]);

  // Back, forward, and a phone's edge swipe all arrive as `popstate`; the URL is the route.
  useEffect(() => {
    const onPop = (): void => {
      const previous = screenRef.current;
      if (previous.name === "player") {
        void api.stopPlayback(previous.ticket.url).catch(() => {});
      }
      showRoute(parseRoute(window.location.pathname));
    };
    window.addEventListener("popstate", onPop);
    return () => window.removeEventListener("popstate", onPop);
  }, [showRoute]);

  const closePlayer = useCallback((id: number): void => {
    if ((window.history.state as { fromApp?: boolean } | null)?.fromApp === true) {
      window.history.back();
    } else {
      const title: Route = { kind: "title", id };
      window.history.replaceState(null, "", routePath(title));
      setScreen({ name: "title", id });
    }
  }, []);

  const tizen = isTizen(navigator.userAgent);
  useSpatialFocus(screen.name !== "loading");
  useTizenFocusPlate(tizen);

  // The Tizen shell navigates away, so the remote's hardware Back key is this client's to handle.
  // At the root there is no earlier screen to show, and Back leaves the app rather than bouncing.
  useTizenBack(tizen, () => {
    const target = backTarget(screenRef.current.name);
    if (target === "exit") return exitTizenApp();
    if (target === "player") {
      const current = screenRef.current;
      if (current.name === "player") {
        void api.stopPlayback(current.ticket.url).catch(() => {});
        closePlayer(current.titleId);
      }
      return true;
    }
    goBack();
    return true;
  });

  // A title page is read from its top however it was entered. A season switch is internal state
  // and keeps its place, so this keys on the title's id rather than on the screen.
  const titleId = screen.name === "title" ? screen.id : null;
  useEffect(() => {
    if (titleId === null) return;
    window.scrollTo(0, 0);
  }, [titleId]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent): void => {
      if (event.key !== "Escape" && event.key !== "Backspace") return;
      if (screen.name === "player") {
        event.preventDefault();
        void api.stopPlayback(screen.ticket.url).catch(() => {});
        closePlayer(screen.titleId);
      } else if (screen.name === "title") {
        event.preventDefault();
        goBack();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [screen, goBack, closePlayer]);

  const openTile = useCallback(
    (tile: Tile): void => {
      push({ kind: "title", id: tile.id });
      setScreen({ name: "title", id: tile.id, preview: tile });
    },
    [push],
  );

  const openId = useCallback(
    (id: number): void => {
      push({ kind: "title", id });
      setScreen({ name: "title", id });
    },
    [push],
  );

  const playTile = useCallback(
    async (tile: Tile): Promise<void> => {
      if (tile.kind === "show") {
        openTile(tile);
        return;
      }
      try {
        // A tile at the very end is not a resume: it would open on the credits.
        const position = tile.percent !== null && tile.percent > 95 ? 0 : tile.positionMs;
        const ticket = await api.playback(tile.id, position);
        setError(null);
        push({ kind: "watch", id: tile.id });
        setScreen({ name: "player", ticket, titleId: tile.id, title: tile.name });
      } catch (caught) {
        setError(text(caught));
      }
    },
    [openTile, push],
  );

  const startPlayback = useCallback(
    (ticket: PlaybackTicket, name: string, titleId: number): void => {
      push({ kind: "watch", id: titleId });
      setScreen({ name: "player", ticket, titleId, title: name });
    },
    [push],
  );

  if (screen.name === "loading") {
    return (
      <main className="screen center">
        <p className="dim">Looking for your library...</p>
      </main>
    );
  }

  if (screen.name === "pair") {
    return (
      <PairScreen
        onPaired={() => {
          void (async () => {
            try {
              const me = await api.me();
              setCanAcquire(me.canAcquire);
            } catch {
              setCanAcquire(false);
            }
            showRoute(parseRoute(window.location.pathname));
          })();
        }}
      />
    );
  }

  if (screen.name === "player") {
    return (
      <Player
        initial={screen.ticket}
        onClose={() => closePlayer(screen.titleId)}
        titleId={screen.titleId}
        titleName={screen.title}
      />
    );
  }

  // Every screen but the player shares one bar; the title page is a tab-less destination.
  const current = screen;
  const activeTab: Tab | null = current.name === "title" ? null : current.name;

  const content = (() => {
    switch (current.name) {
      case "title":
        return (
          <TitleScreen
            canAcquire={canAcquire}
            id={current.id}
            key={current.id}
            onOpen={(tile) => openTile(tile)}
            onPlay={(ticket, name) => startPlayback(ticket, name, current.id)}
            preview={current.preview}
          />
        );
      case "browse":
        return <BrowseScreen onOpen={(tile) => openTile(tile)} />;
      case "myList":
        return <MyListScreen onOpen={(tile) => openTile(tile)} />;
      case "downloads":
        return <DownloadsScreen onOpen={(tile) => openTile(tile)} />;
      case "home":
        return (
          <>
            {error === null ? null : <p className="error">{error}</p>}
            <HomeScreen
              data={home}
              error={homeError}
              loading={homeLoading}
              onOpen={(tile) => openTile(tile)}
              onPlay={(tile) => void playTile(tile)}
              onRetry={() => void loadHome()}
            />
          </>
        );
      case "library":
        return (
          <>
            {error === null ? null : <p className="error">{error}</p>}
            <LibraryScreen error={libraryError} onOpen={openId} rows={rows} />
          </>
        );
    }
  })();

  return (
    <>
      <TopBar
        active={activeTab}
        onBack={current.name === "home" ? null : goBack}
        onBrowse={() => goBrowse()}
        onDownloads={goDownloads}
        onHome={goHome}
        onLibrary={goLibrary}
        onMyList={goMyList}
        onSearch={() => goBrowse(true)}
      />
      {content}
      <TabBar
        active={activeTab}
        onBrowse={() => goBrowse()}
        onDownloads={goDownloads}
        onHome={goHome}
        onLibrary={goLibrary}
        onMyList={goMyList}
      />
    </>
  );
}

type Tab = "home" | "browse" | "myList" | "downloads" | "library";

function TopBar({
  active,
  onBack,
  onHome,
  onBrowse,
  onMyList,
  onDownloads,
  onLibrary,
  onSearch,
}: {
  readonly active: Tab | null;
  readonly onBack: (() => void) | null;
  readonly onHome: () => void;
  readonly onBrowse: () => void;
  readonly onMyList: () => void;
  readonly onDownloads: () => void;
  readonly onLibrary: () => void;
  readonly onSearch: () => void;
}) {
  return (
    <header className="topbar">
      {onBack === null ? null : (
        <button
          aria-label="Back"
          className="nav topbar-back"
          data-focus="true"
          onClick={onBack}
          type="button"
        >
          <ChevronLeftIcon />
        </button>
      )}
      <span className="brand">Peregrine</span>
      <nav className="topbar-nav">
        {(
          [
            ["home", "Home", onHome],
            ["browse", "Browse", onBrowse],
            ["myList", "My List", onMyList],
            ["downloads", "Downloads", onDownloads],
            ["library", "Library", onLibrary],
          ] as const
        ).map(([id, label, action]) => (
          <button
            className={active === id ? "nav current" : "nav"}
            data-focus="true"
            key={id}
            onClick={action}
            type="button"
          >
            {label}
          </button>
        ))}
      </nav>
      <button
        aria-label="Search"
        className="nav search-toggle"
        data-focus="true"
        onClick={onSearch}
        type="button"
      >
        <SearchIcon />
      </button>
      <ProfileMenu />
    </header>
  );
}

function TabBar({
  active,
  onHome,
  onBrowse,
  onMyList,
  onDownloads,
  onLibrary,
}: {
  readonly active: Tab | null;
  readonly onHome: () => void;
  readonly onBrowse: () => void;
  readonly onMyList: () => void;
  readonly onDownloads: () => void;
  readonly onLibrary: () => void;
}) {
  return (
    <nav className="tabbar">
      <button
        className={active === "home" ? "tab current" : "tab"}
        data-focus="true"
        onClick={onHome}
        type="button"
      >
        <HomeIcon />
        <span>Home</span>
      </button>
      <button
        className={active === "browse" ? "tab current" : "tab"}
        data-focus="true"
        onClick={onBrowse}
        type="button"
      >
        <CompassIcon />
        <span>Browse</span>
      </button>
      <button
        className={active === "myList" ? "tab current" : "tab"}
        data-focus="true"
        onClick={onMyList}
        type="button"
      >
        <BookmarkIcon />
        <span>My List</span>
      </button>
      <button
        className={active === "downloads" ? "tab current" : "tab"}
        data-focus="true"
        onClick={onDownloads}
        type="button"
      >
        <DownloadIcon />
        <span>Downloads</span>
      </button>
      <button
        className={active === "library" ? "tab current" : "tab"}
        data-focus="true"
        onClick={onLibrary}
        type="button"
      >
        <LibraryIcon />
        <span>Library</span>
      </button>
    </nav>
  );
}

function LibraryScreen({
  rows,
  error,
  onOpen,
}: {
  readonly rows: readonly LibraryRow[];
  readonly error: string | null;
  readonly onOpen: (id: number) => void;
}) {
  const [query, setQuery] = useState("");
  const [kind, setKind] = useState<"all" | "movie" | "show">("all");
  const [sort, setSort] = useState<"newest" | "oldest" | "title">("newest");
  const visible = useMemo(() => {
    const needle = query.trim().toLocaleLowerCase();
    const filtered = rows.filter((row) => {
      const matchesQuery = needle === "" || row.name.toLocaleLowerCase().includes(needle);
      const matchesKind = kind === "all" || row.kind === (kind === "movie" ? "movies" : "shows");
      return matchesQuery && matchesKind;
    });
    if (sort === "title")
      return filtered.sort((left, right) => left.name.localeCompare(right.name));
    return sort === "newest" ? filtered : filtered.reverse();
  }, [kind, query, rows, sort]);
  useAutoFocus(rows.length);
  return (
    <main className="screen">
      <header className="browse-head">
        <div>
          <h1>Library</h1>
          <p className="dim library-count">
            {visible.length === 1 ? "1 title" : visible.length + " titles"}
          </p>
        </div>
        <form className="search-field" onSubmit={(event) => event.preventDefault()} role="search">
          <SearchIcon />
          <input
            aria-label="Search your library"
            data-focus="true"
            onChange={(event) => setQuery(event.target.value)}
            placeholder="Search your library"
            type="search"
            value={query}
          />
        </form>
      </header>
      <div className="filter-row">
        <div aria-label="Kind" className="segmented" role="group">
          {(["all", "movie", "show"] as const).map((value) => (
            <button
              className={kind === value ? "segment current" : "segment"}
              data-focus="true"
              key={value}
              onClick={() => setKind(value)}
              type="button"
            >
              {value === "all" ? "All" : value === "movie" ? "Films" : "Series"}
            </button>
          ))}
        </div>
        <label className="sort-field">
          <span>Sort</span>
          <select
            data-focus="true"
            onChange={(event) => setSort(event.target.value as typeof sort)}
            value={sort}
          >
            <option value="newest">Newest</option>
            <option value="oldest">Oldest</option>
            <option value="title">Title</option>
          </select>
        </label>
      </div>
      {error === null ? null : <p className="error">{error}</p>}
      {visible.length === 0 ? (
        <p className="dim center-text">
          {rows.length === 0
            ? "The library is empty on this machine."
            : "Nothing matched. Try another title or filter."}
        </p>
      ) : (
        <div className="grid">
          {visible.map((row, index) => (
            <button
              className="tile"
              data-autofocus={index === 0 ? "true" : undefined}
              data-focus="true"
              key={row.id}
              onClick={() => onOpen(row.id)}
              type="button"
            >
              {row.poster === null ? (
                <div className="poster placeholder" />
              ) : (
                <img
                  alt=""
                  className="poster"
                  decoding="async"
                  loading={index < 6 ? "eager" : "lazy"}
                  src={row.poster}
                />
              )}
              {row.rating === null ? null : <span className="chip">{row.rating.toFixed(1)}</span>}
              <span className="tile-title">{row.name}</span>
            </button>
          ))}
        </div>
      )}
    </main>
  );
}

function PairScreen({ onPaired }: { readonly onPaired: () => void }) {
  const [offer, setOffer] = useState<PairOffer | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const done = useRef(false);
  useAutoFocus(offer?.pairId);

  useEffect(() => {
    let cancelled = false;
    const device = describeDevice({
      userAgent: navigator.userAgent,
      platform: navigator.platform,
      maxTouchPoints: navigator.maxTouchPoints,
    });
    void api
      .pair(device.name, device.platform)
      .then((created) => {
        if (!cancelled) setOffer(created);
      })
      .catch((caught: unknown) => {
        if (!cancelled) setMessage(text(caught));
      });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (offer === null || done.current) return;
    const timer = window.setInterval(() => {
      void api
        .pairStatus(offer.pairId)
        .then((status) => {
          if (done.current) return;
          if (status.state === "approved") {
            done.current = true;
            onPaired();
          } else if (status.state === "denied") {
            done.current = true;
            setMessage("The request was refused on the computer. Reload to ask again.");
          } else if (status.state === "unknown") {
            done.current = true;
            setMessage("The code expired. Reload to ask again.");
          }
        })
        .catch(() => {
          // A dropped poll is not a denied pairing; the next tick asks again.
        });
    }, 2000);
    return () => window.clearInterval(timer);
  }, [offer, onPaired]);

  return (
    <main className="screen center">
      <h1>Connect this screen</h1>
      <p className="dim">
        On the computer running Peregrine, open Settings, then Web access, and approve this code.
      </p>
      {offer === null ? (
        <p className="code">{message ?? "Asking for a code..."}</p>
      ) : (
        <p className="code">{offer.code.slice(0, 4) + " " + offer.code.slice(4)}</p>
      )}
      <p className="dim small">Waiting for approval...</p>
    </main>
  );
}

function text(caught: unknown): string {
  return caught instanceof Error ? caught.message : "Something went wrong.";
}
