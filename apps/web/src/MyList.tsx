import { useEffect, useState } from "react";

import { api, type Tile } from "./api.js";
import { useAutoFocus } from "./focus.js";
import { TileCard } from "./Home.js";

/** The account's watchlist, read-only on this surface; adding stays in the desktop app. */
export function MyListScreen({ onOpen }: { readonly onOpen: (tile: Tile) => void }) {
  const [items, setItems] = useState<Tile[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
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

  return (
    <main className="screen my-list-screen">
      <header className="browse-head">
        <div>
          <h1>Saved for later</h1>
        </div>
        <p className="dim">{items.length === 1 ? "1 title" : items.length + " titles"}</p>
      </header>

      {error === null ? null : <p className="error">{error}</p>}
      {loading ? <p className="dim browse-loading">Reading...</p> : null}
      {!loading && items.length === 0 ? (
        <p className="dim browse-empty">
          Nothing on My List yet. Add titles on the desktop app and they appear here.
        </p>
      ) : (
        <div className="browse-grid">
          {items.map((tile, index) => (
            <TileCard autofocus={index === 0} key={tile.id} onOpen={onOpen} tile={tile} />
          ))}
        </div>
      )}
    </main>
  );
}
