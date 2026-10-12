import { clsx } from "clsx";
import type { ReactElement, ReactNode } from "react";
import { twMerge } from "./merge.js";

/**
 * One row inside a `SettingsSection`: a label, an optional line under it, and whatever sits
 * on the trailing edge.
 *
 * It exists so a feature never styles a row of its own (AGENTS R7, PG-E7-03): the padding,
 * the divider it sits on, and the label typography are the kit's, and a screen passes text
 * and a control. `controlId` is how a row labels a control that is not an `<input>` — pass
 * the control's id and the label becomes a `<label for>`, which makes clicking the words
 * operate the control. With no `controlId` the label is plain text, which is right for a row
 * that shows a value rather than one that changes it.
 *
 * The description sits *beside* the label rather than inside it. Inside, it would become
 * part of the control's accessible name — "Start Peregrine when you sign in Peregrine does
 * not register itself unless you ask it to" — which is worse than not being announced at
 * all, and it still reads immediately after the control either way.
 */
export interface SettingsRowProps {
  readonly label: string;
  /**
   * A second line under the label.
   *
   * `| undefined` is deliberate under `exactOptionalPropertyTypes`: a caller forwarding its
   * own optional value — `Switch` does — would otherwise have to build its props
   * conditionally to say the same thing.
   */
  readonly description?: string | undefined;
  /** The id of the control this row's label belongs to, when the row has one. */
  readonly controlId?: string;
  readonly children: ReactNode;
  readonly className?: string;
}

export function SettingsRow({
  label,
  description,
  controlId,
  children,
  className,
}: SettingsRowProps): ReactElement {
  return (
    <div
      className={twMerge(
        clsx("flex w-full items-center justify-between gap-4 px-3 py-2"),
        className,
      )}
    >
      <span className="flex flex-col gap-0.5">
        {controlId === undefined ? (
          <span className="text-body text-text-primary">{label}</span>
        ) : (
          <label className="text-body text-text-primary" htmlFor={controlId}>
            {label}
          </label>
        )}
        {description === undefined ? null : (
          <span className="text-caption text-text-tertiary">{description}</span>
        )}
      </span>
      {children}
    </div>
  );
}
