import { clsx } from "clsx";
import type { ReactElement, ReactNode } from "react";
import { twMerge } from "./merge.js";

/**
 * The icon set (docs/DESIGN_SYSTEM.md §9).
 *
 * One outlined family drawn on a 24-unit grid with a 1.5 px stroke, so every mark keeps
 * its weight when it is scaled to the four sizes the system allows and none looks denser
 * than its neighbour. Geometry is strokes and simple primitives rather than filled shapes,
 * because a filled mark reads as a different family beside an outlined one, and the fill
 * variant belongs to the active state of a control rather than to the icon.
 *
 * The subset is the product's own and it grows with its callers: every name here is drawn
 * somewhere in the shell today, and an icon with no caller would be a mark nobody can
 * review. `ICON_NAMES` is derived from the same table the component reads, so a test that
 * walks it cannot inspect a smaller set than the one that ships.
 */
const PATHS = {
  /** Downloads: an arrow into a tray. */
  download: <path d="M12 3v12m0 0 4-4m-4 4-4-4M4 17v2a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-2" />,

  /** Discover: a compass rose. */
  compass: (
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M15.6 8.4 10.9 10.9 8.4 15.6 13.1 13.1Z" />
    </>
  ),
  /** Browse: four tiles, which is what a filtered grid is. */
  grid: (
    <>
      <rect height="6.5" rx="1.5" width="6.5" x="3.5" y="3.5" />
      <rect height="6.5" rx="1.5" width="6.5" x="14" y="3.5" />
      <rect height="6.5" rx="1.5" width="6.5" x="3.5" y="14" />
      <rect height="6.5" rx="1.5" width="6.5" x="14" y="14" />
    </>
  ),
  /** Edit: a pencil, used beside the selected profile. */
  pencil: (
    <>
      <path d="M4 20h4L18.5 9.5a2.8 2.8 0 0 0-4-4L4 16Z" />
      <path d="m13.5 6.5 4 4" />
    </>
  ),
  /** Close: two strokes crossing, which read the same on all three platforms. */
  close: (
    <>
      <path d="M6 6l12 12" />
      <path d="M18 6L6 18" />
    </>
  ),
  /** Library: spines on a shelf, which read as what is here rather than what could be. */
  library: (
    <>
      <path d="M4 4v16" />
      <path d="M8 4v16" />
      <path d="M12 4v16" />
      <path d="M17.5 5.2 20.4 19" />
    </>
  ),
  /** Diagnostics: a pulse, read from left to right. */
  pulse: <path d="M3 12h3.5l2.5-6 4 12 2.5-6H21" />,
  /** Settings: two sliders, which read as preferences on all three platforms. */
  tune: (
    <>
      <path d="M4 8h9" />
      <circle cx="15" cy="8" r="2" />
      <path d="M18 8h2" />
      <path d="M4 16h4" />
      <circle cx="10" cy="16" r="2" />
      <path d="M13 16h7" />
    </>
  ),
  /** The light theme is one toggle away. */
  sun: (
    <>
      <circle cx="12" cy="12" r="4" />
      <path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4" />
    </>
  ),
  /** The dark theme is one toggle away. */
  moon: <path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z" />,
  /** A rating figure: the one mark that says a number on a badge is a score. */
  star: <path d="m12 3.6 2.6 5.4 5.9.8-4.3 4.1 1.1 5.9-5.3-2.9-5.3 2.9 1.1-5.9-4.3-4.1 5.9-.8Z" />,
  /** One page back, drawn the same way on all three platforms. */
  "chevron-left": <path d="M15 5l-7 7 7 7" />,
  /** One page on. */
  "chevron-right": <path d="M9 5l7 7-7 7" />,
  /** Refresh: a circle broken at its top right, with the head that asks again at the break. */
  refresh: (
    <>
      <path d="M21 12a9 9 0 1 1-2.6-6.4" />
      {/* The head is a V whose vertex is where the arc would have met the top of the circle: a
          corner mark (an `L` of two strokes) reads as a stray bracket at 20 px, which is the size a
          toolbar draws it at. */}
      <path d="M18.9 2.6 21 5.9 17.7 8" />
    </>
  ),
  /** Search: the magnifier, which reads the same way on all three platforms. */
  search: (
    <>
      <circle cx="11" cy="11" r="6.5" />
      {/* The handle starts just inside the lens so the two strokes meet under a round cap rather than
          leaving a notch at the join. */}
      <path d="m15.8 15.8 4.2 4.2" />
    </>
  ),
  /** Play: the triangle every player uses, filled rather than stroked — an outlined one reads as a
      "next" control at 20 px, and a solid mark is what the shape means. */
  play: <path d="M8.4 5.6 18.6 12 8.4 18.4Z" fill="currentColor" />,
  /** Information: the mark a "more" action carries, so the word is not the only signal. */
  info: (
    <>
      <circle cx="12" cy="12" r="8.5" />
      <path d="M12 11.2v4.6" />
      <path d="M12 7.9h.01" />
    </>
  ),
  /** Sidebar: a window with its navigation column marked, which is the shape the control toggles. */
  sidebar: (
    <>
      <rect height="16" rx="2" width="18" x="3" y="4" />
      <path d="M9 4v16" />
    </>
  ),
} as const;

/** The geometry of one icon, as elements inside the shared `svg`. */
type IconGlyph = ReactNode;

/** Every icon the set has. Keys are the only thing a caller may name. */
export type IconName = keyof typeof PATHS;

/**
 * Every icon in the set, so a test walks the whole family rather than a sample.
 *
 * The cast is exact rather than convenient: `IconName` is defined as this object's keys,
 * so the two sets are equal by construction and cannot drift.
 */
export const ICON_NAMES = Object.keys(PATHS) as readonly IconName[];

/**
 * The sizes §9 allows, and the only ones: 16 px beside caption text, 20 px in a row or a
 * toolbar, 24 px standalone, 32 px for an empty state. Off-grid sizes are what make a
 * family look uneven, so the prop is a union rather than a number.
 */
export type IconSize = 16 | 20 | 24 | 32;

export interface IconProps {
  readonly name: IconName;
  readonly size?: IconSize;
  readonly className?: string;
}

/**
 * One icon from the set.
 *
 * The mark is always decorative: it carries no accessible name of its own and is hidden
 * from assistive technology, so the thing that names it is the control or the text beside
 * it. §9 requires every icon to *have* an accessible name, and the way that is enforced is
 * that a control which draws one alone — `IconButton` — takes a `label` that is not
 * optional. An `svg` with its own `<title>` beside a text label announces the label twice.
 *
 * `stroke="currentColor"` is what lets a caller place it in a coloured row without a
 * variant per state, and `focusable="false"` keeps the Windows webview from putting an
 * `svg` in the tab order.
 */
export function Icon({ name, size = 20, className }: IconProps): ReactElement {
  return (
    <svg
      aria-hidden="true"
      className={twMerge(clsx("shrink-0", className))}
      fill="none"
      focusable="false"
      height={size}
      stroke="currentColor"
      strokeLinecap="round"
      strokeLinejoin="round"
      strokeWidth={1.5}
      viewBox="0 0 24 24"
      width={size}
    >
      {PATHS[name] as IconGlyph}
    </svg>
  );
}
