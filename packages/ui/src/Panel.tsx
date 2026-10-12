import { clsx } from "clsx";
import type { ReactElement, ReactNode } from "react";
import { twMerge } from "./merge.js";

/**
 * A bordered surface a screen puts content on.
 *
 * It exists so the surface a block sits on is chosen once: `--surface-1` against the
 * content area's `--surface-0`, a one-pixel `--border-subtle`, and the design system's
 * medium radius (§4). Everything else — padding, dividers, whether it scrolls — is the
 * caller's, because a group of rows and a log viewport want the same surface and different
 * interiors.
 *
 * A shadow is deliberately absent. §4 allows elevation only as a shadow *plus* a border and
 * only for things that float above the page — a menu, a popover, a dialog — and nothing here
 * floats.
 */
export interface PanelProps {
  readonly children: ReactNode;
  readonly className?: string;
}

export function Panel({ children, className }: PanelProps): ReactElement {
  return (
    <div
      className={twMerge(clsx("rounded-md border border-border-subtle bg-surface-1"), className)}
    >
      {children}
    </div>
  );
}
