import { useCallback, useEffect, useId, useRef, useState } from "react";

import { api, type Genre, type Tile } from "./api.js";
import { TileCard } from "./Home.js";

/**
 * The feed under a title, drawn into the page's own scroll.
 *
 * The recommendations are the head; after them the shared genres are paged one provider page at a
 * time, exactly as the desktop title page does. A sentinel below the grid asks for the next page as
 * it nears the viewport, and a second check after each page covers a sentinel that never left it.
 *
 * The section collapses on every viewport. While it is collapsed the sentinel is not mounted and
 * both paging effects stop, so a hidden feed cannot keep asking for pages nobody can see.
 */
export function TitleFeed({
  titleId,
  kind,
  seed,
  genres,
  onOpen,
}: {
  readonly titleId: number;
  readonly kind: "movie" | "show";
  readonly seed: Tile[];
  readonly genres: Genre[];
  readonly onOpen: (tile: Tile) => void;
}) {
  const [items, setItems] = useState<Tile[]>(() => seed.filter((tile) => tile.id !== titleId));
  const [remaining, setRemaining] = useState<Genre[]>(genres);
  const [page, setPage] = useState(1);
  const [loading, setLoading] = useState(false);
  const [failed, setFailed] = useState<string | null>(null);
  const [exhausted, setExhausted] = useState(genres.length === 0);
  const [open, setOpen] = useState(true);
  const inFlight = useRef(false);
  const sentinel = useRef<HTMLDivElement | null>(null);
  const generation = useRef(0);
  const gridId = useId();
  const signature =
    String(titleId) +
    "|" +
    seed
      .slice(0, 1)
      .map((tile) => String(tile.id))
      .join(",") +
    "|" +
    genres.map((genre) => String(genre.id)).join(",");

  useEffect(() => {
    generation.current += 1;
    inFlight.current = false;
    setItems(seed.filter((tile) => tile.id !== titleId));
    setRemaining(genres);
    setPage(1);
    setLoading(false);
    setFailed(null);
    setExhausted(genres.length === 0);
    // The signature is the content of seed and genres, which are new arrays on every read.
  }, [signature]);

  const loadMore = useCallback((): void => {
    const head = remaining[0];
    if (head === undefined || inFlight.current || loading) return;
    const mine = generation.current;
    inFlight.current = true;
    setLoading(true);
    setFailed(null);
    void (async () => {
      try {
        const answer = await api.browse(kind, page, "popularity", head.id);
        if (mine !== generation.current) return;
        setItems((held) => {
          const seen = new Set(held.map((tile) => tile.id));
          const fresh = answer.items.filter((tile) => tile.id !== titleId && !seen.has(tile.id));
          return [...held, ...fresh];
        });
        const drained = !answer.hasMore || answer.items.length === 0;
        if (drained) {
          const rest = remaining.slice(1);
          setRemaining(rest);
          setPage(1);
          if (rest.length === 0) setExhausted(true);
        } else {
          setPage((held) => held + 1);
        }
      } catch (caught) {
        if (mine === generation.current) {
          setFailed(caught instanceof Error ? caught.message : "More titles could not be read.");
        }
      } finally {
        if (mine === generation.current) {
          setLoading(false);
          inFlight.current = false;
        }
      }
    })();
  }, [kind, page, remaining, loading, titleId]);

  useEffect(() => {
    if (!open || loading || exhausted || remaining.length === 0) return;
    const element = sentinel.current;
    if (element === null) return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) loadMore();
      },
      { rootMargin: "600px 0px" },
    );
    observer.observe(element);
    return () => observer.disconnect();
  }, [open, loadMore, loading, exhausted, remaining.length]);

  // A page that lands while the sentinel is still on screen does not change its intersection,
  // so the observer above would never fire again for it. Check the position after every page.
  useEffect(() => {
    if (!open || loading || exhausted || remaining.length === 0) return;
    const element = sentinel.current;
    if (element === null) return;
    if (element.getBoundingClientRect().top < window.innerHeight + 600) loadMore();
  }, [open, items.length, loading, exhausted, remaining.length, loadMore]);

  return (
    <section className="title-feed">
      <div className="section-head">
        <h2>More like this</h2>
        <button
          aria-controls={gridId}
          aria-expanded={open}
          aria-label={open ? "Hide More like this" : "Show More like this"}
          className="collapse"
          data-focus="true"
          onClick={() => setOpen((held) => !held)}
          type="button"
        >
          {open ? "Hide" : "Show"}
        </button>
      </div>
      {open ? (
        <>
          <div className="feed-grid" id={gridId}>
            {items.map((tile) => (
              <TileCard key={tile.id} onOpen={onOpen} tile={tile} watchlisted={tile.watchlisted} />
            ))}
          </div>
          <div aria-hidden="true" className="feed-sentinel" ref={sentinel} />
          {loading ? <p className="dim feed-status">Loading more...</p> : null}
          {failed === null ? null : (
            <div className="feed-status">
              <p className="error">{failed}</p>
              <button className="secondary" data-focus="true" onClick={loadMore} type="button">
                Try again
              </button>
            </div>
          )}
        </>
      ) : null}
    </section>
  );
}
