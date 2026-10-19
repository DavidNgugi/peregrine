import { useEffect, useState } from "react";

import type { HomeData, Rail, Tile } from "./api.js";
import { useIsMobile } from "./useIsMobile.js";

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
  const mobile = useIsMobile();
  const [open, setOpen] = useState(true);
  const collapsible = mobile && rail.items.length > 2;

  return (
    <section className="rail">
      <div className="section-head rail-head">
        <h2>{rail.title}</h2>
        {collapsible ? (
          <button
            aria-expanded={open}
            className="collapse"
            data-focus="true"
            onClick={() => setOpen((held) => !held)}
            type="button"
          >
            {open ? "Hide" : "Show"}
          </button>
        ) : null}
      </div>
      {mobile && !open ? null : (
        <div className="rail-track">
          {rail.items.map((tile) => (
            <TileCard key={tile.id} onOpen={onOpen} tile={tile} />
          ))}
        </div>
      )}
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
  const art = tile.backdrop ?? tile.poster;

  return (
    <section
      className={tile.backdrop === null && tile.poster !== null ? "hero poster-hero" : "hero"}
    >
      {art === null ? null : (
        <img
          alt=""
          className={tile.backdrop === null ? "hero-art hero-art-poster" : "hero-art"}
          decoding="async"
          src={art}
        />
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
  const heroes = data?.heroes?.length ? data.heroes : data?.hero ? [data.hero] : [];
  const [slide, setSlide] = useState(0);
  const [paused, setPaused] = useState(false);
  const current = heroes[slide] ?? heroes[0];

  useEffect(() => {
    setSlide(0);
  }, [data?.hero?.id]);

  useEffect(() => {
    if (heroes.length < 2 || paused) return;
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    const timer = window.setInterval(() => {
      setSlide((held) => (held + 1) % heroes.length);
    }, 8000);
    return () => window.clearInterval(timer);
  }, [heroes.length, paused]);

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
      {current === undefined ? null : (
        <div
          className="hero-carousel"
          onBlur={(event) => {
            if (!event.currentTarget.contains(event.relatedTarget)) setPaused(false);
          }}
          onFocus={() => setPaused(true)}
          onMouseEnter={() => setPaused(true)}
          onMouseLeave={() => setPaused(false)}
        >
          <div className="hero-stage" key={current.id}>
            <Hero onOpen={onOpen} onPlay={onPlay} tile={current} />
          </div>
          {heroes.length > 1 ? (
            <div className="hero-dots" role="group" aria-label="Featured titles">
              {heroes.map((tile, index) => (
                <button
                  aria-label={"Show " + tile.name}
                  aria-pressed={index === slide}
                  className={index === slide ? "hero-dot current" : "hero-dot"}
                  data-focus="true"
                  key={tile.id}
                  onClick={() => setSlide(index)}
                  type="button"
                />
              ))}
            </div>
          ) : null}
        </div>
      )}
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
