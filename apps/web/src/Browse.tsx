import { useEffect, useState } from "react";

import { api, type Genre, type Tile } from "./api.js";
import { TileCard } from "./Home.js";
import { SearchIcon } from "./icons.js";

type Kind = "movie" | "show";

const SORTS = [
  { id: "popularity", label: "Popular" },
  { id: "rating", label: "Top rated" },
  { id: "newest", label: "Newest" },
  { id: "oldest", label: "Oldest" },
] as const;

/**
 * Browse and search: the provider's catalogue when one answers, the library when it does not.
 *
 * Search and the list share one grid. Typing pauses for a third of a second before asking the
 * provider; the kind, sort, and genre controls apply to both paths, so a search is narrowed the
 * same way a browse page is.
 */
export function BrowseScreen({ onOpen }: { readonly onOpen: (tile: Tile) => void }) {
  const [kind, setKind] = useState<Kind>("movie");
  const [sort, setSort] = useState<string>("popularity");
  const [genre, setGenre] = useState<number | undefined>(undefined);
  const [genres, setGenres] = useState<Genre[]>([]);
  const [query, setQuery] = useState("");
  const [search, setSearch] = useState("");
  const [items, setItems] = useState<Tile[]>([]);
  const [page, setPage] = useState(1);
  const [hasMore, setHasMore] = useState(false);
  const [source, setSource] = useState<"provider" | "library">("provider");
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const timer = window.setTimeout(() => {
      setSearch(query.trim());
      setPage(1);
    }, 320);
    return () => window.clearTimeout(timer);
  }, [query]);

  useEffect(() => {
    let cancelled = false;
    void api
      .genres(kind)
      .then((found) => {
        if (!cancelled) setGenres(found);
      })
      .catch(() => {
        if (!cancelled) setGenres([]);
      });
    return () => {
      cancelled = true;
    };
  }, [kind]);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    const run = async (): Promise<void> => {
      try {
        if (search !== "") {
          const found = await api.search(search, kind);
          if (cancelled) return;
          setItems(found);
          setHasMore(false);
          setSource("provider");
        } else {
          const answer = await api.browse(kind, 1, sort, genre);
          if (cancelled) return;
          setItems(answer.items);
          setHasMore(answer.hasMore);
          setSource(answer.source);
        }
      } catch (caught) {
        if (!cancelled) {
          setItems([]);
          setError(caught instanceof Error ? caught.message : "That page could not be read.");
        }
      } finally {
        if (!cancelled) setLoading(false);
      }
    };
    void run();
    return () => {
      cancelled = true;
    };
  }, [kind, sort, genre, search]);

  const loadMore = async (): Promise<void> => {
    const next = page + 1;
    try {
      const answer = await api.browse(kind, next, sort, genre);
      setItems((held) => [...held, ...answer.items]);
      setHasMore(answer.hasMore);
      setPage(next);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : "More titles could not be read.");
    }
  };

  return (
    <main className="screen browse-screen">
      <header className="browse-head">
        <div>
          <h1>Find what to watch next</h1>
        </div>
        <form
          className="search-field"
          onSubmit={(event) => {
            event.preventDefault();
            setSearch(query.trim());
            setPage(1);
          }}
          role="search"
        >
          <SearchIcon />
          <input
            aria-label="Search films and series"
            id="browse-search"
            data-focus="true"
            onChange={(event) => setQuery(event.target.value)}
            placeholder="Search films and series"
            type="search"
            value={query}
          />
        </form>
      </header>

      <div className="filter-row">
        <div className="segmented" role="group" aria-label="Kind">
          {(["movie", "show"] as const).map((value) => (
            <button
              className={kind === value ? "segment current" : "segment"}
              data-focus="true"
              key={value}
              onClick={() => {
                setKind(value);
                setGenre(undefined);
                setPage(1);
              }}
              type="button"
            >
              {value === "movie" ? "Films" : "Series"}
            </button>
          ))}
        </div>
        <label className="sort-field">
          <span>Sort</span>
          <select
            data-focus="true"
            onChange={(event) => {
              setSort(event.target.value);
              setPage(1);
            }}
            value={sort}
          >
            {SORTS.map((held) => (
              <option key={held.id} value={held.id}>
                {held.label}
              </option>
            ))}
          </select>
        </label>
      </div>

      {search === "" && genres.length > 0 ? (
        <div className="genre-row" role="group" aria-label="Genre">
          <button
            className={genre === undefined ? "genre current" : "genre"}
            data-focus="true"
            onClick={() => {
              setGenre(undefined);
              setPage(1);
            }}
            type="button"
          >
            All genres
          </button>
          {genres.map((held) => (
            <button
              className={genre === held.id ? "genre current" : "genre"}
              data-focus="true"
              key={held.id}
              onClick={() => {
                setGenre(held.id);
                setPage(1);
              }}
              type="button"
            >
              {held.name}
            </button>
          ))}
        </div>
      ) : null}

      <p className="browse-note">
        {search === ""
          ? source === "library"
            ? "From your library. Add a metadata provider to browse the wider catalogue."
            : ""
          : 'Results for "' + search + '"'}
      </p>

      {error === null ? null : <p className="error">{error}</p>}
      {loading ? <p className="dim browse-loading">Reading...</p> : null}
      {!loading && items.length === 0 && error === null ? (
        <p className="dim browse-empty">Nothing matched. Try another title or filter.</p>
      ) : (
        <div className="browse-grid">
          {items.map((tile) => (
            <TileCard key={tile.id} onOpen={onOpen} tile={tile} />
          ))}
        </div>
      )}

      {hasMore && search === "" ? (
        <button
          className="secondary browse-more"
          data-focus="true"
          onClick={() => void loadMore()}
          type="button"
        >
          Load more
        </button>
      ) : null}
    </main>
  );
}
