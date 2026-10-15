import { clsx } from "clsx";
import type { CSSProperties, ReactElement } from "react";
import { twMerge } from "./merge.js";

/**
 * A placeholder for content whose geometry is known and whose bytes are not (docs/DESIGN_SYSTEM.md
 * §6 primitives, §11 rule 2).
 *
 * **This is the design system's answer where another application would draw a spinner.** §11 rule 2
 * is "no full-page spinners, ever: cached content or a skeleton with real geometry", and the geometry
 * is the load-bearing half — a row of placeholders the size of the tiles that will replace them means
 * the arrival of the real content moves nothing on the screen. That is also why this component has no
 * width, height, or ratio of its own: the caller states the shape, because only the caller knows what
 * is coming.
 *
 * **The sweep is `transform` on a pseudo-element, and nothing else** (§5 rule 2). It is decoration, so
 * `motion-reduce:animate-none` leaves a still surface, and the *fact* that something is loading is
 * said in words elsewhere (`role="status"`), which is why this is `aria-hidden`: a skeleton is not an
 * announcement.
 */
export interface SkeletonProps {
  readonly className?: string;
  /**
   * The shape, when it comes from a number rather than a class.
   *
   * The kit's own tile sizes are a numeric map (`CARD_WIDTHS`), because Tailwind has to see a class
   * name to emit it and a rail cannot build one from a number — so a caller drawing a tile-shaped
   * placeholder states the width and the ratio here.
   */
  readonly style?: CSSProperties;
}

export function Skeleton({ className, style }: SkeletonProps): ReactElement {
  return (
    <span
      aria-hidden="true"
      className={twMerge(
        clsx(
          "relative block overflow-hidden rounded-md bg-surface-2",
          // The band travels the full width of the shape and is clipped at both ends by
          // `overflow-hidden`, which is what keeps a sweep from painting outside a tile.
          // `after:content-['']` is not optional: a pseudo-element without content is never
          // generated, which is the kit's own idiom (`TitleCard`'s shadow ramp).
          "after:absolute after:inset-0 after:content-[''] after:animate-[shimmer_1.6s_ease-in-out_infinite]",
          // A pass of the text colour at low alpha rather than one surface over another: the theme's
          // surfaces are a low-contrast ladder, so `surface-1` over `surface-2` is a band a reader
          // cannot see, while an alpha of the text colour reads as a sheen in the dark theme and as a
          // shadow in the light one — the direction each theme expects.
          "after:bg-linear-to-r after:from-transparent after:via-text-primary/10 after:to-transparent",
          "motion-reduce:after:animate-none",
          className,
        ),
      )}
      style={style}
    />
  );
}
