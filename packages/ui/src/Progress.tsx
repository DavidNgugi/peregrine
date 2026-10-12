import { clsx } from "clsx";
import type { ReactElement } from "react";
import { twMerge } from "./merge.js";

/**
 * A determinate progress bar (docs/DESIGN_SYSTEM.md §6, primitives).
 *
 * The fill moves with `scaleX` and not with `width`. That is the whole reason this is a
 * component rather than one `div` at each call site: §5 rule 2 allows `transform` and
 * `opacity` and nothing else, because a width animation lays the element out on every frame
 * — and AGENTS §6 forbids an effect that requires layout during interaction. `origin-left`
 * is what makes the scale grow from the start of the track rather than outwards from its
 * centre.
 *
 * The value is clamped rather than trusted: a progress figure arrives over IPC and from a
 * journal row, and a bar asked for 120% would paint outside its track. `role="progressbar"`
 * with `aria-valuenow` is the announced half; the visible label is the caller's, because a
 * screen usually wants to say which phase the figure belongs to.
 */
export interface ProgressProps {
  /** The accessible name of the bar. */
  readonly label: string;
  /** Completion, 0 to 100. Values outside the range are clamped. */
  readonly value: number;
  readonly className?: string;
}

export function Progress({ label, value, className }: ProgressProps): ReactElement {
  const clamped = Math.min(100, Math.max(0, Number.isFinite(value) ? value : 0));

  return (
    <div
      aria-label={label}
      aria-valuemax={100}
      aria-valuemin={0}
      aria-valuenow={Math.round(clamped)}
      className={twMerge(clsx("h-2 w-full overflow-hidden rounded-full bg-surface-2"), className)}
      role="progressbar"
    >
      <div
        className={clsx(
          "h-full origin-left bg-accent",
          "transition-transform duration-200 ease-out motion-reduce:transition-none",
        )}
        style={{ transform: `scaleX(${String(clamped / 100)})` }}
      />
    </div>
  );
}
