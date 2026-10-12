import { clsx } from "clsx";
import type { ReactElement, ReactNode } from "react";
import { twMerge } from "./merge.js";

/**
 * A failure the user can read where it happened (docs/DESIGN_SYSTEM.md §6, collection;
 * PG-E21-19's "errors announced to screen readers").
 *
 * `role="alert"` is what makes it announced when it appears, which is the half of an error
 * a sighted-only treatment loses.
 *
 * The tone is a border rather than the text colour, and that is a contrast decision rather
 * than a stylistic one: §2.2's matrix only holds `--state-danger` to 3:1 — the threshold for
 * a control or an indicator — while body text is held to 4.5:1, so danger-coloured copy is
 * below the documented threshold on some surfaces. A danger border plus primary text clears
 * both, and §2.2's "never colour alone" rule is satisfied by the words themselves.
 */
export interface ErrorStateProps {
  /** What failed, in one line. */
  readonly title: string;
  /** Why, or what to do about it. */
  readonly body?: string;
  /**
   * What the user can do about it, usually one `Button`.
   *
   * It sits outside the text so the control is not part of what is announced as the alert's
   * content, and it is the caller's because only the caller knows whether a failure is
   * dismissible, retryable, or neither.
   */
  readonly action?: ReactNode;
  readonly className?: string;
}

export function ErrorState({ title, body, action, className }: ErrorStateProps): ReactElement {
  return (
    <div
      className={twMerge(
        clsx("flex items-start gap-3 border-l-2 border-state-danger px-3 py-2"),
        className,
      )}
      role="alert"
    >
      <div className="flex flex-1 flex-col gap-1">
        <p className="text-body-strong text-text-primary">{title}</p>
        {body === undefined ? null : <p className="text-caption text-text-secondary">{body}</p>}
      </div>
      {action === undefined ? null : <div className="shrink-0">{action}</div>}
    </div>
  );
}
