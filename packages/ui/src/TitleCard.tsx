import { cva } from "class-variance-authority";
import { clsx } from "clsx";
import { useCallback, useEffect, useState, type ReactElement, type TransitionEvent } from "react";
import { Icon } from "./Icon.js";
import { Link } from "./Link.js";
import {
  badgeVisible,
  DWELL_MS,
  IDLE_PREVIEW,
  MIN_VISIBLE_RATIO,
  REVEAL_LIFT_Z,
  nextPreview,
  panelVisible,
  previewLift,
  previewMotion,
  previewScale,
  type PreviewEvent,
  type PreviewState,
} from "./hover-preview.js";
import { twMerge } from "./merge.js";

/**
 * A poster tile: one title, at a fixed 2:3 ratio, with its score, what is happening to it, and the
 * hover preview that `PG-E7-05` is judged on.
 *
 * Four decisions are load-bearing here, and each of them is why a rule below looks the way it does.
 *
 * **The box is reserved before the bytes arrive.** §4's tile model is a fixed aspect ratio, and the
 * geometry comes from the box rather than from the picture: the frame is `aspect-[2/3]` at a width the
 * size variant fixes, so a poster that loads late, a poster that fails, and a poster the cache has
 * never held all occupy the same rectangle. That is what keeps a grid of five thousand tiles still
 * while its images stream in, and it is the acceptance this component was written against.
 *
 * **Nothing here is drawn that a poster could be confused for.** The tile carries no visible title
 * text: the name is in the accessibility tree, and what the eye gets instead is the metadata layer
 * §5's ladder reveals (`PG-E7-05`) — a caption under every poster would be a second, competing
 * source of truth for the title's name.
 *
 * **The ladder is a pure state machine and this component is its wiring.** The pointer, the timer,
 * the `IntersectionObserver`, and `transitionend` are all events; `hover-preview.ts` decides what
 * they mean, and the tests there are the ones that can hold §5 to the letter, because this file's
 * halves are the ones a Node test cannot reach (`PG-E21-03`).
 *
 * **No layout thrash, which is why the layer is inside fixed bounds.** The card never changes size:
 * the revealed panel is absolutely positioned at the poster's foot. Only `transform` and
 * `opacity` animate, the shadow ramp rides a pseudo-element's opacity rather than an
 * animated `box-shadow`, and `will-change` is on for exactly the card that is transitioning and off
 * every other tile in the grid (§5 rule 3). `contain: layout style` — and deliberately not `paint`,
 * which would clip the ramp's own shadow to the tile's box.
 *
 * **The kit holds no copy.** Every word on this tile comes from the caller — the availability chip's
 * label, the rating's spoken form, the vote count as the caller formatted it, the Details verb, and
 * the one-line meta — because `packages/ui` is a leaf with no i18n (AGENTS R7 keeps the rule, not the
 * words, out of the UI). The badge's *tone* is the kit's, since a state colour is a token.
 */

/** The five tile widths. All multiples of 8, so a rail of them sits on §4's space rhythm. */
export const CARD_SIZES = ["xs", "sm", "md", "lg", "xl"] as const;

export type CardSize = (typeof CARD_SIZES)[number];

/**
 * The width each size variant draws, in px.
 *
 * The numbers are stated twice — here and in the `w-[…]` classes below — because Tailwind's scanner
 * reads class names and a template string is not one, and the duplication is held together by a test
 * that renders every size and compares what it drew with this map. It has to be a map at all because
 * the rail cannot ask a tile its size: it renders its items through the caller's function, so the
 * room it leaves for the hover ladder is the room the *largest* tile needs (`revealRoom`).
 */
export const CARD_WIDTHS: Readonly<Record<CardSize, number>> = {
  xs: 88,
  sm: 112,
  md: 144,
  lg: 176,
  xl: 216,
};

/** §4's tile ratio, width ÷ height, which the frame's `aspect-[2/3]` draws. */
export const CARD_ASPECT = 2 / 3;

/**
 * Availability, spelled exactly as `peregrine_domain::availability::Availability::as_str` spells it.
 *
 * The union is the core's vocabulary rather than the kit's: a second spelling here would be a mapping
 * to maintain and a silent mismatch the day the core gains a state. The chip is drawn only for the
 * three states below whose signal is not `null`, which is a display decision and lives with the tile.
 */
export type CardAvailability =
  | "discovered"
  | "watchlisted"
  | "wanted"
  | "acquiring"
  | "available"
  | "partial"
  | "missing"
  | "unavailable"
  | "removed";

/**
 * The dot colour each state carries, or `null` for a state that draws no chip.
 *
 * `null` is the quiet answer and it is deliberate for four of the nine: a title nobody has asked for,
 * one that is merely on a watchlist, one that is on disk, and one that was there and is now gone from
 * the catalogue are all states a poster should not wear a badge for. "Removed" is the arguable one and
 * it keeps its chip, because a poster for something the library no longer holds is exactly the case a
 * person needs told. Colour never carries a state alone: the chip's word does (§2.2).
 */
const AVAILABILITY_SIGNALS: Readonly<Record<CardAvailability, string | null>> = {
  discovered: null,
  watchlisted: null,
  wanted: "bg-border-strong",
  acquiring: "bg-state-info",
  available: null,
  partial: "bg-state-warn",
  missing: "bg-state-warn",
  unavailable: "bg-state-danger",
  removed: "bg-border-strong",
};

/**
 * What a title's download is doing, as the card draws it.
 *
 * The three states are the app's view vocabulary rather than the domain's ten (see
 * `apps/desktop/src/lib/download-status.ts`); the *tone* is the kit's, because a status colour is
 * a token and a caller that picked its own would be the second place the palette lives.
 */
export type CardDownloadStatus = "active" | "done" | "failed";

/**
 * The border each download status draws across the tile's top edge.
 *
 * A border rather than a second chip because it is a glanceable signal over a wall of posters: a
 * reader scanning Browse can see which titles are already coming without reading each tile. The
 * rule that colour never carries a state alone still holds — the caller's word goes into the
 * tile's accessible name below, and the border itself is decorative.
 */
const DOWNLOAD_SIGNALS: Readonly<Record<CardDownloadStatus, string>> = {
  active: "bg-accent",
  done: "bg-state-ok",
  failed: "bg-state-danger",
};

/**
 * Whether the image layer is drawn at all.
 *
 * A pure function because the failing half cannot be reached from a static render and a rule with no
 * test is a rule nobody has run: a poster that failed to decode must fall back to the placeholder —
 * an evicted blob, a path the CSP refuses, and a server error are all one `onError` — while a missing
 * or empty URL is the placeholder from the start.
 *
 * It narrows as well as answers, so the `src` below cannot be handed a `null` the compiler would have
 * to be told to ignore.
 */
export function posterVisible(url: string | null | undefined, failed: boolean): url is string {
  return url !== null && url !== undefined && url !== "" && !failed;
}

/** A score to one decimal, which is how a rating is written everywhere it appears. */
export function formatRating(rating: number): string {
  return rating.toFixed(1);
}

/**
 * The synopsis as the panel draws it, or `null` when there is nothing to draw.
 *
 * Whitespace is collapsed because a provider's prose arrives with its own newlines and the panel is a
 * clamped block, not a document. The three-line clamp itself is CSS (`line-clamp-3`), which drops
 * whole lines at a word boundary: the kit never cuts a string mid-word and never builds an ellipsis
 * out of the pieces, because "The Matrix is a…sci" is worse than saying less.
 */
export function previewOverview(overview: string | null | undefined): string | null {
  if (overview === null || overview === undefined) {
    return null;
  }
  const text = overview.trim().replace(/\s+/gu, " ");
  return text === "" ? null : text;
}

/**
 * A caller's line, or `null` when it says nothing.
 *
 * Absent, `null`, and whitespace are one answer — "there is no line here" — so the panel and the
 * accessible name cannot disagree about whether a word was given, and a chip with a dot and no word
 * cannot be drawn (§2.2: colour never carries a state alone).
 */
function said(value: string | null | undefined): string | null {
  if (value === null || value === undefined) {
    return null;
  }
  const text = value.trim();
  return text === "" ? null : text;
}

/**
 * The shadow ramp, which is the one part of the ladder a static render can see.
 *
 * §2.2's snippet is the shape: the `::after` carries the shadow and a single opacity transition for
 * both directions, so the shadow itself is never animated — only the layer's opacity is, which is a
 * compositor property. `motion-reduce:transition-none` is the kit's established reduced-motion
 * branch (`Button`, `Progress`, `Sidebar`), and it is what makes the reveal *instant* rather than
 * absent: the panel still appears, it just stops moving. The translation's own duration and easing
 * are §5's two ends and live in `hover-preview.ts`, because which end applies is a state decision.
 */
const RAMP =
  "after:opacity-0 after:transition-opacity motion-reduce:after:transition-none " +
  "after:duration-[var(--dur-3,180ms)] after:ease-[var(--ease-standard,cubic-bezier(0.2,0,0,1))]";

const cardVariants = cva(
  [
    // The rounded frame and its border are the card's at every size (§4: "cards use the same radius
    // and shadow at every size"). `contain: layout style` rather than the `layout paint style` §2.2
    // suggests: paint containment clips a descendant's painting to the tile's box, and the shadow
    // ramp *is* a descendant — its whole point is the part that falls outside the border box. What
    // is clipped instead is the poster, by the inner frame.
    "group relative block rounded-md border border-border-subtle bg-surface-2",
    "[contain:layout_style]",
    "transition-transform motion-reduce:transition-none",
    // §5's shadow ramp: a pseudo-element's opacity, never an animated box-shadow. `pointer-events:
    // none` because it covers the tile and the tile is a link.
    "after:pointer-events-none after:absolute after:inset-0 after:rounded-[inherit] after:content-['']",
    "after:shadow-[0_12px_32px_rgb(0_0_0_/_0.45)]",
    RAMP,
    "focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus-ring",
  ],
  {
    variants: {
      size: {
        xs: "w-[88px]",
        sm: "w-[112px]",
        md: "w-[144px]",
        lg: "w-[176px]",
        xl: "w-[216px]",
      },
    },
    defaultVariants: { size: "md" },
  },
);

export interface TitlePreviewPanelProps {
  /** The score the panel repeats beside its star, when the catalogue holds one. */
  readonly rating?: number | null;
  /**
   * The title's name, when the tile was asked to draw one.
   *
   * `null` is the ordinary case, and the reason is in the module comment: a provider's poster *is* the
   * title's name, composed for the box, and a caption under it is a second source of truth for the same
   * fact. A captured frame is not — see `TitleCardProps::showTitle`.
   */
  readonly name?: string | null;
  /** The caller's one line — "1999 · Film" — which is also the tile's accessible name. */
  readonly meta?: string | null;
  /** The synopsis the list answer carried. Whitespace-collapsed and clamped to three lines. */
  readonly overview?: string | null;
  /** The Details verb, localized. Without it the panel draws no action. */
  readonly detailsLabel?: string | null;
}

/**
 * The metadata layer the ladder reveals at the foot of the enlarged poster.
 *
 * **It is not a control surface.** PG-E9-01 owns the watchlist and bookmark actions, and an enabled
 * button that does nothing is worse than an absent one — the rule PG-E1-07 already set for the menu
 * — so the one action here is Details, and it is a *span inside the tile's link* rather than a button
 * or a nested anchor: the whole tile navigates, the rail stays one tab stop (§2.7), and a focusable
 * control inside a focusable link would be an invalid nesting and a second stop for the same
 * destination.
 *
 * **It is hidden from assistive technology, and deliberately.** Everything it shows is either
 * already in the tile's accessible name — the meta line and the rating's spoken form — or, for the
 * synopsis, prose a person chose to hover for; announcing it again inside the link would make the
 * link's name a paragraph.
 *
 * **The vote count is gone, and that is a decision rather than an omission.** A tile is judged by its
 * artwork and its score; how many people voted on a title thousands of strangers also rated is noise
 * on every tile in a rail, and it was the longest string the panel drew.
 */
export function TitlePreviewPanel({
  rating,
  name,
  meta,
  overview,
  detailsLabel,
}: TitlePreviewPanelProps): ReactElement {
  const prose = previewOverview(overview);
  const showsScore = rating !== null && rating !== undefined;
  const metaLine = said(meta);
  const details = said(detailsLabel);

  return (
    <span
      aria-hidden="true"
      className={clsx(
        "absolute inset-x-0 bottom-0 flex max-h-full flex-col justify-end gap-1 overflow-hidden rounded-b-[inherit]",
        // The theme surface pairs with its text in both themes; the media scrim is dark in both.
        "bg-linear-to-b from-transparent via-surface-1 via-20% to-surface-1 p-2 pt-4 text-caption text-text-primary",
      )}
      data-preview-panel=""
    >
      {showsScore ? (
        <span className="flex flex-wrap items-baseline gap-x-2 gap-y-0.5">
          <span className="inline-flex items-center gap-0.5 tabular-nums">
            <Icon className="text-state-warn" name="star" size={16} />
            {formatRating(rating)}
          </span>
        </span>
      ) : null}
      {/* The name, when the caller asked for it — see `TitleCardProps::showTitle`. `truncate` rather than
          a wrap, because a wrapped line inside a panel that is pinned to the poster's foot pushes the meta
          and the action out of the visible box, which is what it did before this was clamped. */}
      {name === null ? null : <span className="truncate font-medium">{name}</span>}
      {metaLine === null ? null : <span className="truncate text-text-secondary">{metaLine}</span>}
      {prose === null ? null : <span className="line-clamp-3">{prose}</span>}
      {details === null ? null : (
        <span className="mt-0.5 inline-flex w-full items-center justify-center rounded-full bg-accent px-2 py-1 font-medium text-text-on-accent">
          {details}
        </span>
      )}
    </span>
  );
}

export interface TitleCardProps {
  /** Where the tile goes. The kit has no router; the app's route table builds this. */
  readonly href: string;
  /** The title's name. Drawn only for assistive technology — see the module comment. */
  readonly title: string;
  /** One line the caller composes, such as "1979 · Film". In the accessible name and the panel. */
  readonly meta?: string;
  /** The cached poster, as the asset protocol's URL. `null` draws the placeholder. */
  readonly poster?: string | null;
  /**
   * The placeholder's colour, from the title's blurhash (PG-E6-11).
   *
   * Absent is a neutral surface rather than an invented colour: a tile with no placeholder data has
   * no business guessing at the poster's dominant hue.
   */
  readonly placeholder?: string;
  /** The score out of ten, as the catalogue holds it. Absent draws no badge. */
  readonly rating?: number | null;
  /** How the rating is spoken, such as "rated 8.5 out of 10". The number alone is not a name. */
  readonly ratingLabel?: string | null;
  /** The synopsis the list answer carried, drawn in the revealed panel's three clamped lines. */
  readonly overview?: string | null;
  /** The Details verb, localized. Absent draws no action in the panel. */
  readonly detailsLabel?: string | null;
  /**
   * Whether the container left room for the tile's enlargement on hover.
   *
   * True for a rail, which reserves it through `revealRoom`; **false for a grid**, whose columns are
   * derived to fit the measured width and therefore have no room to give. A grid that scaled anyway had
   * its outer tiles cut off by the scroller on both sides, which is what a screenshot of a fresh library
   * showed.
   */
  readonly lift?: boolean;
  /**
   * Whether the revealed panel draws the title's name.
   *
   * **A per-surface decision, and there is exactly one surface on the other side of it.** A provider's
   * poster identifies its title — composed for the 2:3 box, and what a reader recognises — so a rail of
   * them says nothing with a caption the artwork did not already say. A **captured frame** is a
   * photograph of one moment of a film: it identifies nothing, and a library tiled with them is a wall a
   * reader cannot read. So the library asks for the name and the rails do not.
   */
  readonly showTitle?: boolean;
  readonly availability?: CardAvailability;
  /** The chip's word, localized by the caller. Without it the state draws no chip. */
  readonly availabilityLabel?: string;
  /**
   * The title's download state, drawn as a border across the tile's top edge.
   *
   * Absent, `null`, or without {@link TitleCardProps.downloadLabel} draws nothing: the kit holds no
   * copy, and a colour with no word beside it would carry the state alone (§2.2). The two travel
   * together so a status cannot be shown without the name a screen reader reads.
   */
  readonly downloadStatus?: CardDownloadStatus | null;
  /** The status's word, localized by the caller. Drawn only in the accessible name. */
  readonly downloadLabel?: string | null;
  /**
   * How far an in-flight transfer has got, as whole percent.
   *
   * Drawn as a translucent overlay only while {@link TitleCardProps.downloadStatus} is `active`; a
   * number without a status would be a progress bar for something the card cannot name.
   */
  readonly downloadProgress?: number | null;
  /**
   * Whether the title is on the account's watchlist, drawn as a bookmark on the poster.
   *
   * A mark rather than a chip: it is a fact about the tile the reader chose, not a state the
   * catalogue is in, and it must not compete with the availability chip's colour. The word beside
   * it is the caller's, because the kit holds no copy.
   */
  readonly watchlisted?: boolean;
  /** The watchlist mark's word, for the accessible name. */
  readonly watchlistLabel?: string | null;
  readonly size?: CardSize;
  /**
   * Loads eagerly and at high priority.
   *
   * §2.5 is explicit that the first rail and the hero are not lazy-loaded, and this is how a caller
   * says which tiles those are: everything below the fold stays `lazy`/`low`, which is what keeps a
   * large grid from asking for artwork nobody has scrolled to.
   */
  readonly priority?: boolean;
  /**
   * Where the tile sits in its rail's single tab stop (§2.7).
   *
   * A rail moves this between tiles as the arrows do, which is why it is a prop at all: an `<a href>`
   * is focusable by default, so a tile outside a rail needs to set nothing.
   */
  readonly tabIndex?: number;
  readonly onNavigate?: () => void;
  readonly className?: string;
}

export function TitleCard({
  href,
  title,
  meta,
  poster,
  placeholder,
  rating,
  ratingLabel,
  overview,
  detailsLabel,
  showTitle = false,
  availability,
  availabilityLabel,
  downloadStatus,
  downloadLabel,
  downloadProgress,
  watchlisted = false,
  watchlistLabel,
  size,
  // The container's answer to the tile's enlargement. A rail reserves the room and a grid cannot,
  // so the name is the room's rather than the interaction's: `previewLift` below is the stacking
  // order a revealed tile takes, and the two are different questions.
  lift: reserved = true,
  priority = false,
  tabIndex,
  onNavigate,
  className,
}: TitleCardProps): ReactElement {
  const [decoded, setDecoded] = useState(false);
  const [failed, setFailed] = useState(false);
  const [preview, setPreview] = useState<PreviewState>(IDLE_PREVIEW);
  const [focused, setFocused] = useState(false);
  // The element the observer watches. It arrives from the pointer event rather than from a ref
  // because `Link` forwards none — and because the card only needs it once an interaction has
  // started, which is exactly when a pointer is there to hand it over.
  const [node, setNode] = useState<HTMLElement | null>(null);

  const dispatch = useCallback((event: PreviewEvent) => {
    setPreview((current) => nextPreview(current, event));
  }, []);

  const active = preview.phase !== "idle";

  // §5's dwell. The timer is armed by *being* in the dwelling phase and cleared by leaving it, so a
  // cancel of any kind disarms it without anyone having to remember to.
  useEffect(() => {
    if (preview.phase !== "dwelling") {
      return;
    }
    const timer = window.setTimeout(() => {
      dispatch({ type: "dwellElapsed" });
    }, DWELL_MS);
    return () => {
      window.clearTimeout(timer);
    };
  }, [preview.phase, dispatch]);

  // The two global cancels. Both are attached only while an interaction is in flight: a listener per
  // tile on a five-thousand-tile grid would be five thousand listeners for something one pointer can
  // do at a time. The scroll listener is passive and capturing, because a scroll inside a rail does
  // not bubble and capturing is what hears it.
  useEffect(() => {
    if (!active) {
      return;
    }
    const onScroll = (): void => {
      dispatch({ type: "scroll" });
    };
    const onVisibility = (): void => {
      dispatch({ type: "visibilityChange", hidden: document.hidden });
    };
    document.addEventListener("scroll", onScroll, { capture: true, passive: true });
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      document.removeEventListener("scroll", onScroll, { capture: true });
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [active, dispatch]);

  // §5's visibility floor. A card scrolling out from under the pointer never gets a leave, and the
  // observer's first callback is what answers the case where the tile was already below the floor
  // when the pointer arrived.
  useEffect(() => {
    if (!active || node === null || typeof IntersectionObserver === "undefined") {
      return;
    }
    const observer = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          dispatch({ type: "intersection", ratio: entry.intersectionRatio });
        }
      },
      { threshold: [MIN_VISIBLE_RATIO] },
    );
    observer.observe(node);
    return () => {
      observer.disconnect();
    };
  }, [active, node, dispatch]);

  const signal = availability === undefined ? null : AVAILABILITY_SIGNALS[availability];
  // An empty word is no word: a chip with a dot and nothing else would carry its state by colour
  // alone, which §2.2 forbids, so the state draws nothing rather than a swatch.
  const chip = said(availabilityLabel);
  const showsChip = signal !== null && chip !== null;

  // A download mark is drawn only when a status and its word are both present, for the reason the
  // chip is: a colour alone would carry the state, and the kit refuses to invent the word.
  const downloadSignal =
    downloadStatus === undefined || downloadStatus === null
      ? null
      : DOWNLOAD_SIGNALS[downloadStatus];
  const downloadWord = said(downloadLabel);
  const showsDownload = downloadSignal !== null && downloadWord !== null;
  const percent = downloadProgress ?? null;
  const showsProgress = downloadStatus === "active" && percent !== null;

  const metaLine = said(meta);
  const watchWord = watchlisted ? said(watchlistLabel) : null;
  const spoken = [
    title,
    watchWord,
    metaLine,
    said(ratingLabel),
    showsChip ? chip : null,
    showsDownload ? downloadWord : null,
    showsProgress && percent !== null ? `${String(percent)}%` : null,
  ]
    .filter((part): part is string => part !== null)
    .join(", ");

  const revealed = preview.phase === "revealed";
  const motion = previewMotion(preview);
  const showsPanel = panelVisible(preview, focused);
  const lift = previewLift(preview);

  /** `transitionend` on the card's own transform is the moment §5 rule 3 turns the promotion off. */
  const onTransitionEnd = (event: TransitionEvent<HTMLAnchorElement>): void => {
    // The shadow ramp's opacity transition ends here too (as `::after`), and the poster's fade is a
    // child's — neither is the interaction settling.
    if (event.propertyName !== "transform" || event.pseudoElement !== "") {
      return;
    }
    dispatch({ type: "settled" });
  };

  return (
    <Link
      className={twMerge(
        clsx(
          cardVariants({ size }),
          revealed ? "after:opacity-100" : "after:opacity-0",
          "hover:no-underline focus-visible:outline-offset-2",
        ),
        className,
      )}
      href={href}
      onBlur={() => {
        setFocused(false);
      }}
      onFocus={() => {
        setFocused(true);
      }}
      onPointerEnter={(event) => {
        setNode(event.currentTarget);
        dispatch({ type: "pointerEnter" });
      }}
      onPointerLeave={() => {
        dispatch({ type: "pointerLeave" });
      }}
      onTransitionEnd={onTransitionEnd}
      style={{
        transform: `scale(${String(previewScale(preview, reserved))})`,
        // §5's two ends, from the state. Inline rather than as classes so the enter and the exit can
        // differ without two `duration-*` utilities racing in the cascade — and so `will-change` is
        // a value that is *absent*, not an empty rule, on every tile that is not moving.
        transitionDuration: motion.duration,
        transitionTimingFunction: motion.easing,
        ...(preview.animating ? { willChange: "transform" } : {}),
        // The lift, for the reason `REVEAL_LIFT_Z` gives: without it the ramp of a tile that has a
        // later sibling is painted under that sibling, which is half the lift `PG-E7-05` is judged
        // on. `null` is document order, which is what every tile that is not enlarged uses.
        ...(lift === null && !focused ? {} : { zIndex: lift ?? REVEAL_LIFT_Z }),
      }}
      {...(tabIndex === undefined ? {} : { tabIndex })}
      {...(onNavigate === undefined ? {} : { onNavigate })}
    >
      <span className="sr-only">{spoken}</span>
      {showsDownload ? (
        <span
          aria-hidden="true"
          className={clsx(
            "absolute inset-x-0 top-0 z-10 h-1.5 rounded-t-[inherit]",
            downloadSignal,
          )}
        />
      ) : null}
      <span
        className="relative block aspect-[2/3] w-full overflow-hidden rounded-[inherit] bg-surface-2"
        {...(placeholder === undefined ? {} : { style: { backgroundColor: placeholder } })}
      >
        {showsProgress && percent !== null ? (
          <span
            aria-hidden="true"
            className="absolute inset-0 z-20 grid place-items-center bg-black/55 backdrop-blur-[1px]"
            data-download-overlay=""
          >
            <span className="text-lg font-semibold tabular-nums text-white">{percent}%</span>
          </span>
        ) : null}
        {posterVisible(poster, failed) ? (
          <img
            alt=""
            aria-hidden="true"
            className={twMerge(
              clsx(
                "absolute inset-0 size-full object-cover transition-opacity motion-reduce:transition-none",
                decoded ? "opacity-100" : "opacity-0",
              ),
            )}
            decoding="async"
            fetchPriority={priority ? "high" : "low"}
            loading={priority ? "eager" : "lazy"}
            onError={() => {
              setFailed(true);
            }}
            onLoad={() => {
              setDecoded(true);
            }}
            src={poster}
          />
        ) : null}
      </span>
      {watchlisted ? (
        <span
          aria-hidden="true"
          className={clsx(
            "absolute top-1 left-1 z-10 inline-flex items-center justify-center rounded-sm border",
            "border-border-subtle bg-surface-0 p-0.5 text-text-primary",
          )}
          data-watchlisted=""
        >
          <Icon name="bookmark" size={16} />
        </span>
      ) : null}
      {rating === undefined || rating === null || !badgeVisible(preview, focused) ? null : (
        <span className="absolute bottom-1 left-1">
          <span
            aria-hidden="true"
            className={clsx(
              "inline-flex items-center gap-0.5 rounded-sm border border-border-subtle",
              "bg-surface-0 px-1 py-0.5 text-caption text-text-primary tabular-nums",
            )}
          >
            <Icon className="text-state-warn" name="star" size={16} />
            {formatRating(rating)}
          </span>
        </span>
      )}
      {showsChip ? (
        // The chip is pinned to the tile's own edges and truncates rather than overflowing it: the
        // smallest size is 88 px and a localized word can be long (§3 holds layouts to 1.4×
        // expansion), so a chip sized to its text would run off the poster and be clipped mid-word.
        // The dot survives the truncation, which is what keeps the state from being colour alone.
        <span
          aria-hidden="true"
          className={clsx(
            "absolute top-1 right-1 inline-flex max-w-[calc(100%-0.5rem)] items-center gap-1",
            "rounded-sm border border-border-subtle bg-surface-0 px-1 py-0.5",
            "text-caption text-text-primary",
          )}
        >
          <span className={clsx("size-1.5 shrink-0 rounded-full", signal)} />
          <span className="truncate">{chip}</span>
        </span>
      ) : null}
      {showsPanel ? (
        <TitlePreviewPanel
          detailsLabel={said(detailsLabel)}
          meta={metaLine}
          name={showTitle ? title : null}
          overview={overview ?? null}
          rating={rating ?? null}
        />
      ) : null}
    </Link>
  );
}
