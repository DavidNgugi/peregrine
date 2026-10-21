import { useEffect, useState } from "react";

import { api, type Tile } from "./api.js";
import { useAutoFocus } from "./focus.js";
import { TileCard } from "./Home.js";

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
  useAutoFocus(items[0]?.id);

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
        <p className="dim">{items.length === 1 ? "1 title" : items.length + " titles"}</p>
      </header>

      {error === null ? null : <p className="error">{error}</p>}
      {actionError === null ? null : (
        <p className="error" role="alert">
          {actionError}
        </p>
      )}
      {loading ? <p className="dim browse-loading">Reading...</p> : null}
      {!loading && items.length === 0 ? (
        <p className="dim browse-empty">
          Nothing on My List yet. Add titles on the desktop app and they appear here.
        </p>
      ) : (
        <div className="browse-grid">
          {items.map((tile, index) => (
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
