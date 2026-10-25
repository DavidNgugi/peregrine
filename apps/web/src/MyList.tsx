import { useEffect, useMemo, useState } from "react";

import { api, type Tile } from "./api.js";
import { useAutoFocus } from "./focus.js";
import { TileCard } from "./Home.js";
import { SearchIcon } from "./icons.js";

/**
 * The account's watchlist: a title is added or removed from its own page, and this screen offers
 * the remove action beside each card. The control is a sibling of the card rather than a child,
 * because the card itself is one button and a button inside a button is neither valid markup nor
 * one tab stop.
 */
export function MyListScreen({ onOpen }: { readonly onOpen: (tile: Tile) => void }) {
  const [items, setItems] = useState<Tile[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [removing, setRemoving] = useState<number | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [kind, setKind] = useState<"all" | "movie" | "show">("all");
  const [sort, setSort] = useState<"newest" | "oldest" | "title">("newest");
  useAutoFocus(items[0]?.id);

  const visible = useMemo(() => {
    const needle = query.trim().toLocaleLowerCase();
    const filtered = items.filter((item) => {
      const matchesQuery = needle === "" || item.name.toLocaleLowerCase().includes(needle);
      const matchesKind = kind === "all" || item.kind === kind;
      return matchesQuery && matchesKind;
    });
    if (sort === "title")
      return filtered.sort((left, right) => left.name.localeCompare(right.name));
    return sort === "newest" ? filtered : filtered.reverse();
  }, [items, kind, query, sort]);

  useEffect(() => {
    let cancelled = false;
    void api
      .watchlist()
      .then((found) => {
        if (!cancelled) {
          setItems(found);
          setLoading(false);
        }
      })
      .catch((caught: unknown) => {
        if (!cancelled) {
          setError(caught instanceof Error ? caught.message : "My List could not be read.");
          setLoading(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const remove = async (tile: Tile): Promise<void> => {
    setRemoving(tile.id);
    setActionError(null);
    try {
      await api.watchlistSet(tile.id, false);
      setItems((held) => held.filter((item) => item.id !== tile.id));
    } catch (caught) {
      setActionError(
        caught instanceof Error ? caught.message : "That title could not be removed from My List.",
      );
    } finally {
      setRemoving(null);
    }
  };

  return (
    <main className="screen my-list-screen">
      <header className="browse-head">
        <div>
          <h1>Saved for later</h1>
        </div>
        <form className="search-field" onSubmit={(event) => event.preventDefault()} role="search">
          <SearchIcon />
          <input
            aria-label="Search My List"
            data-focus="true"
            onChange={(event) => setQuery(event.target.value)}
            placeholder="Search saved titles"
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
      {actionError === null ? null : (
        <p className="error" role="alert">
          {actionError}
        </p>
      )}
      {loading ? <p className="dim browse-loading">Reading...</p> : null}
      {!loading && visible.length === 0 ? (
        <p className="dim browse-empty">
          {items.length === 0
            ? "Nothing on My List yet. Add titles on the desktop app and they appear here."
            : "Nothing matched. Try another title or filter."}
        </p>
      ) : (
        <div className="browse-grid">
          {visible.map((tile, index) => (
            <div className="card-cell" key={tile.id}>
              <TileCard
                autofocus={index === 0}
                onOpen={onOpen}
                priority={index < 6}
                tile={tile}
                watchlisted={tile.watchlisted}
              />
              <button
                aria-label={"Remove " + tile.name + " from My List"}
                className="card-remove"
                data-focus="true"
                disabled={removing === tile.id}
                onClick={() => void remove(tile)}
                type="button"
              >
                {removing === tile.id ? "Removing..." : "Remove"}
              </button>
            </div>
          ))}
        </div>
      )}
    </main>
  );
}
