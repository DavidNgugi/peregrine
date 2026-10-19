/**
 * The ten-foot and phone client: pair, home, library, a title's page, and the player (ADR 0028).
 *
 * The shape is Netflix's, reduced to what this core actually holds: a hero over horizontal rails,
 * a title page with episodes for a series and More Like This for anything the provider can name,
 * and a player that writes the same progress row the desktop player writes.
 */

import { useCallback, useEffect, useRef, useState } from "react";

import {
  api,
  ApiError,
  type LibraryRow,
  type PairOffer,
  type PlaybackTicket,
  type Tile,
} from "./api.js";
import { BrowseScreen } from "./Browse.js";
import { DownloadsScreen } from "./Downloads.js";
import { useAutoFocus, useSpatialFocus } from "./focus.js";
import { HomeScreen } from "./Home.js";
import {
  BookmarkIcon,
  CompassIcon,
  DownloadIcon,
  HomeIcon,
  LibraryIcon,
  SearchIcon,
} from "./icons.js";
import { MyListScreen } from "./MyList.js";
import { Player } from "./Player.js";
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
  const [returnTo, setReturnTo] = useState<"home" | "browse" | "myList" | "downloads" | "library">(
    "home",
  );
  const [libraryError, setLibraryError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const loadHome = useCallback(async (): Promise<void> => {
    setHomeLoading(true);
    try {
      const data = await api.home();
      setHome(data);
      setHomeError(null);
      setScreen({ name: "home" });
    } catch (caught) {
      if (caught instanceof ApiError && caught.status === 401) {
        setScreen({ name: "pair" });
        return;
      }
      setHomeError(text(caught));
      setScreen({ name: "home" });
    } finally {
      setHomeLoading(false);
    }
  }, []);

  const loadLibrary = useCallback(async (): Promise<void> => {
    try {
      const library = await api.library();
      setRows(library);
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

  useEffect(() => {
    void loadHome();
  }, [loadHome]);

  const goHome = useCallback((): void => {
    setScreen({ name: "home" });
    void loadHome();
  }, [loadHome]);

  const goLibrary = useCallback((): void => {
    void loadLibrary();
  }, [loadLibrary]);

  const goBrowse = useCallback((focusSearch = false): void => {
    setScreen({ name: "browse" });
    if (focusSearch) {
      window.setTimeout(() => document.getElementById("browse-search")?.focus(), 60);
    }
  }, []);

  const goMyList = useCallback((): void => {
    setScreen({ name: "myList" });
  }, []);

  const goDownloads = useCallback((): void => {
    setScreen({ name: "downloads" });
  }, []);

  const goBack = useCallback((): void => {
    switch (returnTo) {
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
        void loadLibrary();
        break;
      case "home":
        goHome();
        break;
    }
  }, [returnTo, goHome, loadLibrary]);

  useSpatialFocus(screen.name !== "loading");

  useEffect(() => {
    const onKey = (event: KeyboardEvent): void => {
      if (event.key !== "Escape" && event.key !== "Backspace") return;
      if (screen.name === "player") {
        event.preventDefault();
        setScreen({ name: "title", id: screen.titleId });
      } else if (screen.name === "title") {
        event.preventDefault();
        goBack();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [screen, goBack]);

  const openTile = useCallback(
    (tile: Tile, from: "home" | "browse" | "myList" | "downloads" | "library" = "home"): void => {
      setReturnTo(from);
      setScreen({ name: "title", id: tile.id, preview: tile });
    },
    [],
  );

  const openId = useCallback((id: number): void => {
    setReturnTo("library");
    setScreen({ name: "title", id });
  }, []);

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
        setScreen({ name: "player", ticket, titleId: tile.id, title: tile.name });
      } catch (caught) {
        setError(text(caught));
      }
    },
    [openTile],
  );

  const startPlayback = useCallback(
    (ticket: PlaybackTicket, name: string, titleId: number): void => {
      setScreen({ name: "player", ticket, titleId, title: name });
    },
    [],
  );

  if (screen.name === "loading") {
    return (
      <main className="screen center">
        <p className="dim">Looking for your library...</p>
      </main>
    );
  }

  if (screen.name === "pair") {
    return <PairScreen onPaired={() => void loadHome()} />;
  }

  if (screen.name === "player") {
    return (
      <Player
        initial={screen.ticket}
        onClose={() => setScreen({ name: "title", id: screen.titleId })}
        titleId={screen.titleId}
        titleName={screen.title}
      />
    );
  }

  if (screen.name === "title") {
    return (
      <>
        <TitleScreen
          id={screen.id}
          key={screen.id}
          onBack={goBack}
          onOpen={(tile) => openTile(tile, returnTo)}
          onPlay={(ticket, name) => startPlayback(ticket, name, screen.id)}
          preview={screen.preview}
        />
        <TabBar
          active={null}
          onBrowse={() => goBrowse()}
          onHome={goHome}
          onDownloads={goDownloads}
          onLibrary={goLibrary}
          onMyList={goMyList}
        />
      </>
    );
  }

  if (screen.name === "browse") {
    return (
      <>
        <TopBar
          active="browse"
          onBrowse={() => goBrowse()}
          onHome={goHome}
          onDownloads={goDownloads}
          onLibrary={goLibrary}
          onMyList={goMyList}
          onSearch={() => goBrowse(true)}
        />
        <BrowseScreen onOpen={(tile) => openTile(tile, "browse")} />
        <TabBar
          active="browse"
          onBrowse={() => goBrowse()}
          onHome={goHome}
          onDownloads={goDownloads}
          onLibrary={goLibrary}
          onMyList={goMyList}
        />
      </>
    );
  }

  if (screen.name === "myList") {
    return (
      <>
        <TopBar
          active="myList"
          onBrowse={() => goBrowse()}
          onHome={goHome}
          onDownloads={goDownloads}
          onLibrary={goLibrary}
          onMyList={goMyList}
          onSearch={() => goBrowse(true)}
        />
        <MyListScreen onOpen={(tile) => openTile(tile, "myList")} />
        <TabBar
          active="myList"
          onBrowse={() => goBrowse()}
          onHome={goHome}
          onDownloads={goDownloads}
          onLibrary={goLibrary}
          onMyList={goMyList}
        />
      </>
    );
  }

  if (screen.name === "downloads") {
    return (
      <>
        <TopBar
          active="downloads"
          onBrowse={() => goBrowse()}
          onDownloads={goDownloads}
          onHome={goHome}
          onLibrary={goLibrary}
          onMyList={goMyList}
          onSearch={() => goBrowse(true)}
        />
        <DownloadsScreen onOpen={(tile) => openTile(tile, "downloads")} />
        <TabBar
          active="downloads"
          onBrowse={() => goBrowse()}
          onDownloads={goDownloads}
          onHome={goHome}
          onLibrary={goLibrary}
          onMyList={goMyList}
        />
      </>
    );
  }

  return (
    <>
      <TopBar
        active={screen.name}
        onBrowse={() => goBrowse()}
        onHome={goHome}
        onDownloads={goDownloads}
        onLibrary={goLibrary}
        onMyList={goMyList}
        onSearch={() => goBrowse(true)}
      />
      {error === null ? null : <p className="error">{error}</p>}
      {screen.name === "home" ? (
        <HomeScreen
          data={home}
          error={homeError}
          loading={homeLoading}
          onOpen={(tile) => openTile(tile, "home")}
          onPlay={(tile) => void playTile(tile)}
          onRetry={() => void loadHome()}
        />
      ) : (
        <LibraryScreen error={libraryError} onOpen={openId} rows={rows} />
      )}
      <TabBar
        active={screen.name}
        onBrowse={() => goBrowse()}
        onHome={goHome}
        onDownloads={goDownloads}
        onLibrary={goLibrary}
        onMyList={goMyList}
      />
    </>
  );
}

type Tab = "home" | "browse" | "myList" | "downloads" | "library";

function TopBar({
  active,
  onHome,
  onBrowse,
  onMyList,
  onDownloads,
  onLibrary,
  onSearch,
}: {
  readonly active: Tab;
  readonly onHome: () => void;
  readonly onBrowse: () => void;
  readonly onMyList: () => void;
  readonly onDownloads: () => void;
  readonly onLibrary: () => void;
  readonly onSearch: () => void;
}) {
  return (
    <header className="topbar">
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
  useAutoFocus(rows.length);
  return (
    <main className="screen">
      <p className="dim library-count">{rows.length === 1 ? "1 title" : rows.length + " titles"}</p>
      {error === null ? null : <p className="error">{error}</p>}
      {rows.length === 0 ? (
        <p className="dim center-text">The library is empty on this machine.</p>
      ) : (
        <div className="grid">
          {rows.map((row, index) => (
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
                <img alt="" className="poster" decoding="async" loading="lazy" src={row.poster} />
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
    void api
      .pair("Peregrine TV", navigator.userAgent.slice(0, 32))
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
