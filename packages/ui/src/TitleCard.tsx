import { cva } from "class-variance-authority";
import { clsx } from "clsx";
import { useState, type ReactElement } from "react";
import { Icon } from "./Icon.js";
import { Link } from "./Link.js";
import { twMerge } from "./merge.js";

/**
 * A poster tile: one title, at a fixed 2:3 ratio, with its score and what is happening to it.
 *
 * Three decisions are load-bearing here, and each of them is why a rule below looks the way it does.
 *
 * **The box is reserved before the bytes arrive.** §4's tile model is a fixed aspect ratio, and the
 * geometry comes from the box rather than from the picture: the frame is `aspect-[2/3]` at a width the
 * size variant fixes, so a poster that loads late, a poster that fails, and a poster the cache has
 * never held all occupy the same rectangle. That is what keeps a grid of five thousand tiles still
 * while its images stream in, and it is the acceptance this component was written against.
 *
 * **Nothing here is drawn that a poster could be confused for.** The tile carries no title text: §5's
 * hover-preview ladder puts metadata inside the scaled layer (PG-E7-05), and a caption under every
 * poster would be a second, competing source of truth for the title's name. What the tile does carry
 * is the accessible name, as text a screen reader reads and the eye does not.
 *
 * **The kit holds no copy.** Every word on this tile comes from the caller — the availability chip's
 * label and the rating's spoken form are props — because `packages/ui` is a leaf with no i18n and the
 * application's catalogue is where a sentence is translated (AGENTS R7 keeps the rule, not the words,
 * out of the UI). The badge's *tone* is the kit's, since a state colour is a token.
 */

/** The five tile widths. All multiples of 8, so a rail of them sits on §4's space rhythm. */
export const CARD_SIZES = ["xs", "sm", "md", "lg", "xl"] as const;

export type CardSize = (typeof CARD_SIZES)[number];

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

const cardVariants = cva(
  [
    // The rounded frame and its border are the card's at every size (§4: "cards use the same radius
    // and shadow at every size"). `contain` is the structural hint §2.2 asks for rather than a
    // `will-change`, which across a large grid is a layer per tile and hundreds of megabytes.
    "group relative block overflow-hidden rounded-md border border-border-subtle bg-surface-2",
    "[contain:layout_paint_style]",
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

export interface TitleCardProps {
  /** Where the tile goes. The kit has no router; the app's route table builds this. */
  readonly href: string;
  /** The title's name. Drawn only for assistive technology — see the module comment. */
  readonly title: string;
  /** One line the caller composes, such as "1979 · Film". Drawn only for assistive technology. */
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
  readonly ratingLabel?: string;
  readonly availability?: CardAvailability;
  /** The chip's word, localized by the caller. Without it the state draws no chip. */
  readonly availabilityLabel?: string;
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
  availability,
  availabilityLabel,
  size,
  priority = false,
  tabIndex,
  onNavigate,
  className,
}: TitleCardProps): ReactElement {
  const [decoded, setDecoded] = useState(false);
  const [failed, setFailed] = useState(false);

  const signal = availability === undefined ? null : AVAILABILITY_SIGNALS[availability];
  // An empty word is no word: a chip with a dot and nothing else would carry its state by colour
  // alone, which §2.2 forbids, so the state draws nothing rather than a swatch.
  const showsChip =
    signal !== null && availabilityLabel !== undefined && availabilityLabel !== "";

  const spoken = [title, meta, ratingLabel, showsChip ? availabilityLabel : undefined]
    .filter((part): part is string => part !== undefined && part !== "")
    .join(", ");

  return (
    <Link
      className={twMerge(
        clsx(cardVariants({ size }), "hover:no-underline focus-visible:outline-offset-2"),
        className,
      )}
      href={href}
      {...(tabIndex === undefined ? {} : { tabIndex })}
      {...(onNavigate === undefined ? {} : { onNavigate })}
    >
      <span className="sr-only">{spoken}</span>
      <span
        className="relative block aspect-[2/3] w-full bg-surface-2"
        {...(placeholder === undefined ? {} : { style: { backgroundColor: placeholder } })}
      >
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
      {rating === undefined || rating === null ? null : (
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
          <span className="truncate">{availabilityLabel}</span>
        </span>
      ) : null}
    </Link>
  );
}
