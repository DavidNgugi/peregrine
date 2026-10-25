import { useCallback, useEffect, useId, useState, type TransitionEvent } from "react";

import type { HomeData, Rail, Tile } from "./api.js";
import {
  DWELL_MS,
  IDLE_PREVIEW,
  MIN_VISIBLE_RATIO,
  nextPreview,
  panelVisible,
  previewScale,
  type PreviewEvent,
  type PreviewState,
} from "./hover-preview.js";
import { useAutoFocus } from "./focus.js";
import { BookmarkIcon, StarIcon } from "./icons.js";
import { isTizen } from "./tizen.js";

/** The one input the ladder answers to: a mouse or trackpad, not a remote or a finger. */
const FINE_POINTER = "(hover: hover) and (pointer: fine)";

function finePointer(): boolean {
  return typeof window !== "undefined" && window.matchMedia(FINE_POINTER).matches;
}

/** The facts under a title's name: what it is, and when it came out. */
function tileFacts(tile: Tile): string {
  return [
    tile.kind === "show" ? "Series" : "Film",
    typeof tile.year === "number" ? String(tile.year) : null,
  ]
    .filter((part): part is string => part !== null)
    .join("  ·  ");
}

/** The synopsis as the panel draws it: provider prose, whitespace-collapsed, or nothing. */
function panelOverview(tile: Tile): string | null {
  if (tile.overview === null) return null;
  const text = tile.overview.trim().replace(/\s+/gu, " ");
  return text === "" ? null : text;
}

/**
 * One poster card in a rail or grid.
 *
 * The tile is one button, and the hover preview is a layer inside it: the state machine decides
 * when the layer is drawn and how far the tile has grown, while this component only forwards the
 * pointer, timer, observer, and transition events. The panel is `aria-hidden` because every word
 * in it is either already in the tile's accessible name or prose a person chose to reveal.
 *
 * The scale is applied only on a fine pointer. A remote, a keyboard, and a touch screen get the
 * panel on focus without moving the tile, which is the keyboard parity §5 requires.
 */
export function TileCard({
  tile,
  onOpen,
  autofocus = false,
  priority = false,
  watchlisted = tile.watchlisted,
}: {
  readonly tile: Tile;
  readonly onOpen: (tile: Tile) => void;
  readonly autofocus?: boolean;
  /** Whether the first visible row should load its poster eagerly. */
  readonly priority?: boolean;
  readonly watchlisted?: boolean;
}) {
  const [preview, setPreview] = useState<PreviewState>(IDLE_PREVIEW);
  const [focused, setFocused] = useState(false);
  // The input type is a property of the device, and a listener per tile on a grid of thousands is
  // the cost the reference card avoids. Navigating remounts the grid if the input ever changes.
  // The television has no pointer: the focus panel is the only reveal, and a hover-preview
  // transform on a tile the remote just left is the "stuck highlight" the reference guide warns
  // about. This is evaluated once per grid, not per focus move.
  const [hoverable] = useState(() => finePointer() && !isTizen(navigator.userAgent));
  const [node, setNode] = useState<HTMLElement | null>(null);

  const dispatch = useCallback((event: PreviewEvent) => {
    setPreview((current) => nextPreview(current, event));
  }, []);

  const active = preview.phase !== "idle";

  // §5's dwell: armed by being in the dwelling phase and disarmed by leaving it, so every cancel
  // clears the timer without a second place to remember.
  useEffect(() => {
    if (!hoverable || preview.phase !== "dwelling") return;
    const timer = window.setTimeout(() => {
      dispatch({ type: "dwellElapsed" });
    }, DWELL_MS);
    return () => window.clearTimeout(timer);
  }, [hoverable, preview.phase, dispatch]);

  // Reduced motion removes the transform transition, and the phone grid removes the transform
  // itself; neither ever fires the transitionend the ladder settles on. Settle straight away so an
  // exit cannot leave the lift and will-change behind.
  useEffect(() => {
    if (!preview.animating) return;
    const instant =
      window.matchMedia("(prefers-reduced-motion: reduce)").matches ||
      !window.matchMedia("(min-width: 721px)").matches;
    if (!instant) return;
    dispatch({ type: "settled" });
  }, [preview.animating, dispatch]);

  // The two global cancels, attached only while an interaction is in flight: one listener per tile
  // on a grid of hundreds would be hundreds for something one pointer can do at a time. The scroll
  // listener is passive and capturing so a scroll inside a scroll container is heard too.
  useEffect(() => {
    if (!active) return;
    const onScroll = (): void => dispatch({ type: "scroll" });
    const onVisibility = (): void =>
      dispatch({ type: "visibilityChange", hidden: document.hidden });
    document.addEventListener("scroll", onScroll, { capture: true, passive: true });
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      document.removeEventListener("scroll", onScroll, { capture: true });
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [active, dispatch]);

  // §5's visibility floor. A tile that scrolls out from under a still pointer never receives a
  // leave; the observer's first callback also answers a tile that was already below the floor.
  useEffect(() => {
    if (!active || node === null || typeof IntersectionObserver === "undefined") return;
    const observer = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          dispatch({ type: "intersection", ratio: entry.intersectionRatio });
        }
      },
      { threshold: [MIN_VISIBLE_RATIO] },
    );
    observer.observe(node);
    return () => observer.disconnect();
  }, [active, node, dispatch]);

  const showsPanel = panelVisible(preview, focused);
  const scale = hoverable ? previewScale(preview) : 1;
  const settling = preview.phase === "idle" && preview.animating;
  const facts = tileFacts(tile);
  const overview = panelOverview(tile);

  const onTransitionEnd = (event: TransitionEvent<HTMLButtonElement>): void => {
    // The name's opacity and the panel's own animation end here too; neither is the tile settling.
    if (event.propertyName !== "transform" || event.pseudoElement !== "") return;
    dispatch({ type: "settled" });
  };

  return (
    <button
      className="card"
      data-autofocus={autofocus ? "true" : undefined}
      data-focus="true"
      data-focus-key={"tile:" + String(tile.id)}
      data-panel={showsPanel ? "true" : undefined}
      data-preview={settling ? "settling" : preview.phase}
      onClick={() => onOpen(tile)}
      onBlur={() => setFocused(false)}
      onFocus={() => setFocused(true)}
      onPointerEnter={
        hoverable
          ? (event) => {
              setNode(event.currentTarget);
              dispatch({ type: "pointerEnter" });
            }
          : undefined
      }
      onPointerLeave={hoverable ? () => dispatch({ type: "pointerLeave" }) : undefined}
      onTransitionEnd={onTransitionEnd}
      style={{
        ...(scale === 1 ? {} : { transform: "scale(" + String(scale) + ")" }),
        ...(preview.animating ? { willChange: "transform" } : {}),
      }}
      type="button"
    >
      <span className="card-art">
        {tile.poster === null ? (
          <span className="card-placeholder" />
        ) : (
          <img alt="" decoding="async" loading={priority ? "eager" : "lazy"} src={tile.poster} />
        )}
        {tile.percent === null || tile.percent <= 0 ? null : (
          <span className="card-progress">
            <i style={{ width: Math.min(100, tile.percent) + "%" }} />
          </span>
        )}
        {tile.rating === null ? null : (
          <span className="card-rating">
            <StarIcon />
            {tile.rating.toFixed(1)}
          </span>
        )}
        {watchlisted ? (
          <span aria-hidden="true" className="card-bookmark">
            <BookmarkIcon />
          </span>
        ) : null}
        {showsPanel ? (
          <span aria-hidden="true" className="card-preview">
            <span className="card-preview-name">{tile.name}</span>
            {facts === "" ? null : <span className="card-preview-meta">{facts}</span>}
            {tile.rating === null ? null : (
              <span className="card-preview-rating">
                <StarIcon />
                {tile.rating.toFixed(1)}
              </span>
            )}
            {overview === null ? null : <span className="card-preview-overview">{overview}</span>}
          </span>
        ) : null}
      </span>
      <span className="card-name">{tile.name}</span>
    </button>
  );
}

/** One rail of tiles, a vertical grid that can be collapsed on every viewport. */
export function RailRow({
  rail,
  onOpen,
}: {
  readonly rail: Rail;
  readonly onOpen: (tile: Tile) => void;
}) {
  const [open, setOpen] = useState(true);
  const panelId = useId();
  const collapsible = rail.items.length > 0;

  return (
    <section className="rail">
      <div className="section-head rail-head">
        <h2>{rail.title}</h2>
        {collapsible ? (
          <button
            aria-controls={panelId}
            aria-expanded={open}
            aria-label={(open ? "Hide " : "Show ") + rail.title}
            className="collapse"
            data-focus="true"
            onClick={() => setOpen((held) => !held)}
            type="button"
          >
            {open ? "Hide" : "Show"}
          </button>
        ) : null}
      </div>
      {open ? (
        <div className="rail-track" id={panelId}>
          {rail.items.map((tile, index) => (
            <TileCard
              key={tile.id}
              onOpen={onOpen}
              priority={index < 6}
              tile={tile}
              watchlisted={tile.watchlisted}
            />
          ))}
        </div>
      ) : null}
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
  const facts = tileFacts(tile);
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
        <p className="hero-meta">
          {facts}
          {tile.rating === null ? null : (
            <span className="hero-rating">
              <StarIcon />
              {tile.rating.toFixed(1)}
            </span>
          )}
        </p>
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

/** The home screen: a hero and vertical rails, each of which collapses on any viewport. */
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

  // The hero's Details/Play button is the entry point. The key is the first hero's id rather
  // than the rotating slide, so a carousel tick never steals focus from the rails.
  useAutoFocus(data?.hero?.id);

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
