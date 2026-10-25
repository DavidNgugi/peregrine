import { clsx } from "clsx";
import { useEffect, type ReactElement, type ReactNode } from "react";
import { Button } from "./Button.js";
import { twMerge } from "./merge.js";

/** How a toast reads: a success, a failure, or work in progress. */
export type ToastTone = "success" | "danger" | "progress";

export interface ToastProps {
  /** The one line that names what happened. */
  readonly title: string;
  /** Why, or what to do next. */
  readonly body?: string;
  /** Whether the toast is news, a failure, or work still running. */
  readonly tone: ToastTone;
  /** Dismisses it; the caller owns visibility so a new result can replace an old one. */
  readonly onDismiss?: (() => void) | undefined;
  /** The accessible name of the dismiss control. */
  readonly dismissLabel?: string | undefined;
  /** How long it stays before onDismiss runs; absent means it waits for the caller. */
  readonly autoDismissMs?: number | undefined;
  /** A control beside the copy, such as Retry. */
  readonly action?: ReactNode;
  readonly className?: string;
}

/**
 * A transient result that is visible wherever the person is on the screen.
 *
 * **A settings action and its answer can be a screen apart.** The Integrations section is long,
 * and a job that starts under the fold used to finish in a sentence at the top of the section —
 * technically present, practically invisible. This is the same answer drawn where the eye is.
 *
 * role=alert on a failure and role=status otherwise is the same split the rest of the kit uses:
 * a failure is announced, a result is announced politely, and a progress line does not interrupt.
 * The border carries the tone; the copy stays primary text because --state-danger is a 3:1
 * indicator colour, not body text (DESIGN_SYSTEM §2.2).
 */
export function Toast({
  title,
  body,
  tone,
  onDismiss,
  dismissLabel = "Dismiss",
  autoDismissMs,
  action,
  className,
}: ToastProps): ReactElement {
  useEffect(() => {
    if (autoDismissMs === undefined || onDismiss === undefined) {
      return undefined;
    }
    const timer = window.setTimeout(onDismiss, autoDismissMs);
    return () => {
      window.clearTimeout(timer);
    };
  }, [autoDismissMs, onDismiss]);

  const toneClass =
    tone === "danger"
      ? "border-l-state-danger"
      : tone === "success"
        ? "border-l-state-ok"
        : "border-l-state-info";

  return (
    <div
      // The host is a fixed layer above the page, not a layout participant: it must not move a
      // Settings row when it appears. pointer-events-none lets the page below stay usable.
      aria-live={tone === "danger" ? "assertive" : "polite"}
      className="pointer-events-none fixed inset-x-0 bottom-6 z-50 flex justify-center px-4"
      role={tone === "danger" ? "alert" : "status"}
    >
      <div
        className={twMerge(
          clsx(
            "pointer-events-auto flex w-full max-w-md items-start gap-3 rounded-lg border border-border-subtle bg-surface-1 px-4 py-3 shadow-elev-3",
            "border-l-2",
            toneClass,
          ),
          className,
        )}
      >
        <div className="flex min-w-0 flex-1 flex-col gap-1">
          <p className="text-body-strong text-text-primary">{title}</p>
          {body === undefined ? null : <p className="text-caption text-text-secondary">{body}</p>}
        </div>
        {action === undefined ? null : <div className="shrink-0">{action}</div>}
        {onDismiss === undefined ? null : (
          <Button onClick={onDismiss} size="sm" variant="ghost">
            {dismissLabel}
          </Button>
        )}
      </div>
    </div>
  );
}
