import { clsx } from "clsx";
import type { ReactElement } from "react";
import { twMerge } from "./merge.js";

/**
 * A state with nothing to show (docs/DESIGN_SYSTEM.md §6, collection; §11 rule 2).
 *
 * It is deliberately not a spinner and it does not place itself: §11 rules a full-page
 * spinner out, and whether this block is centred in a page or one row of a settings group
 * is the container's business, not the block's. What it owns is the shape every empty state
 * shares — one line saying what is not there, and an optional second line saying why or what
 * to do next. There is no illustration, because the branding that would draw one is
 * PG-E22-01's, and an empty state waiting for artwork is an empty state nobody ships.
 *
 * The body is optional on purpose: §13.1's notification-centre empty state is a single line,
 * and a component that required a second one would push filler copy into a screen to satisfy
 * it.
 */
export interface EmptyStateProps {
  /** What is not here. */
  readonly title: string;
  /** Why, or what to do about it. */
  readonly body?: string;
  readonly className?: string;
}

export function EmptyState({ title, body, className }: EmptyStateProps): ReactElement {
  return (
    <div className={twMerge(clsx("flex flex-col gap-1 px-3 py-4"), className)}>
      <p className="text-body-strong text-text-secondary">{title}</p>
      {body === undefined ? null : <p className="text-caption text-text-tertiary">{body}</p>}
    </div>
  );
}
