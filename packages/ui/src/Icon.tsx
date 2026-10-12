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
  /** Discover: a compass rose. */
  compass: (
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M15.6 8.4 10.9 10.9 8.4 15.6 13.1 13.1Z" />
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
