import { useEffect } from "react";

import type { HomeData, Rail, Tile } from "./api.js";
import { useAutoFocus } from "./focus.js";

/** One poster card in a rail or grid. */
export function TileCard({
  tile,
  onOpen,
  autofocus = false,
}: {
  readonly tile: Tile;
  readonly onOpen: (tile: Tile) => void;
  readonly autofocus?: boolean;
}) {
  return (
    <button
      className="card"
      data-autofocus={autofocus ? "true" : undefined}
      data-focus="true"
      onClick={() => onOpen(tile)}
      type="button"
    >
      <span className="card-art">
        {tile.poster === null ? (
          <span className="card-placeholder" />
        ) : (
          <img alt="" decoding="async" loading="lazy" src={tile.poster} />
        )}
        {tile.percent === null || tile.percent <= 0 ? null : (
          <span className="card-progress">
            <i style={{ width: Math.min(100, tile.percent) + "%" }} />
          </span>
        )}
        {tile.rating === null ? null : (
          <span className="card-rating">{tile.rating.toFixed(1)}</span>
        )}
      </span>
      <span className="card-name">{tile.name}</span>
    </button>
  );
}

/** One horizontal rail of tiles. */
export function RailRow({
  rail,
  onOpen,
}: {
  readonly rail: Rail;
  readonly onOpen: (tile: Tile) => void;
}) {
  return (
    <section className="rail">
      <h2>{rail.title}</h2>
      <div className="rail-track">
        {rail.items.map((tile) => (
          <TileCard key={tile.id} onOpen={onOpen} tile={tile} />
        ))}
      </div>
    </section>
  );
}

/** The hero: one title, its backdrop, and the two buttons a home screen leads with. */
function Hero({
  tile,
  onOpen,
  onPlay,
}: {
  readonly tile: Tile;
  readonly onOpen: (tile: Tile) => void;
  readonly onPlay: (tile: Tile) => void;
}) {
  const meta = [
    tile.kind === "show" ? "Series" : "Film",
    typeof tile.year === "number" ? String(tile.year) : null,
    tile.rating === null ? null : tile.rating.toFixed(1),
  ]
    .filter((part): part is string => part !== null)
    .join("  ·  ");

  return (
    <section className="hero">
      {tile.backdrop === null ? null : (
        <img alt="" className="hero-art" decoding="async" src={tile.backdrop} />
      )}
      <div className="hero-copy">
        <p className="hero-meta">{meta}</p>
        <h1>{tile.name}</h1>
        {tile.overview === null ? null : <p className="hero-overview">{tile.overview}</p>}
        <div className="actions">
          {tile.playable ? (
            <button
              className="play"
              data-autofocus="true"
              data-focus="true"
              onClick={() => onPlay(tile)}
              type="button"
            >
              {tile.percent !== null && tile.percent > 0 ? "Resume" : "Play"}
            </button>
          ) : null}
          <button
            className="secondary"
            data-autofocus={tile.playable ? undefined : "true"}
            data-focus="true"
            onClick={() => onOpen(tile)}
            type="button"
          >
            {tile.playable ? "More info" : "Details"}
          </button>
        </div>
      </div>
    </section>
  );
}

/** The home screen: a hero and horizontal rails, scrolled by touch or the D-pad. */
export function HomeScreen({
  data,
  loading,
  error,
  onOpen,
  onPlay,
  onRetry,
}: {
  readonly data: HomeData | null;
  readonly loading: boolean;
  readonly error: string | null;
  readonly onOpen: (tile: Tile) => void;
  readonly onPlay: (tile: Tile) => void;
  readonly onRetry: () => void;
}) {
  useAutoFocus(data?.hero?.id ?? data?.rails[0]?.items[0]?.id);

  // The provider rails are filled by a job the first read starts. One quiet retry closes the gap
  // between "fetching" and the rails appearing, without a spinner over the local ones.
  useEffect(() => {
    if (data?.state !== "fetching") return;
    const timer = window.setTimeout(onRetry, 5000);
    return () => window.clearTimeout(timer);
  }, [data?.state, onRetry]);

  if (data === null) {
    return (
      <main className="screen center">
        <p className="dim">
          {error ?? (loading ? "Building your home..." : "Nothing to show yet.")}
        </p>
        {error === null ? null : (
          <button className="secondary" data-focus="true" onClick={onRetry} type="button">
            Try again
          </button>
        )}
      </main>
    );
  }

  return (
    <main className="screen home">
      {data.hero === null ? null : <Hero onOpen={onOpen} onPlay={onPlay} tile={data.hero} />}
      {data.rails.length === 0 ? (
        <p className="dim center-text">
          Scan a folder in the desktop app and its titles appear here.
        </p>
      ) : (
        data.rails.map((rail) => <RailRow key={rail.id} onOpen={onOpen} rail={rail} />)
      )}
      {data.state === "fetching" ? (
        <p className="caption">Fetching new titles from your provider...</p>
      ) : null}
      {data.state === "no_provider" ? (
        <p className="caption">Add a metadata provider in Settings to fill the discovery rails.</p>
      ) : null}
    </main>
  );
}
