import { clsx } from "clsx";
import { useId } from "react";
import type { ReactElement } from "react";
import { twMerge } from "./merge.js";
import { SettingsRow } from "./SettingsRow.js";

/**
 * A settings row whose control is an on/off switch (docs/DESIGN_SYSTEM.md §6, primitives).
 *
 * It is a row rather than a bare control because of the target: the track is 36×20, which
 * is under the 28 px pointer minimum of §4, so the button carries the padding that takes it
 * to 28 px tall and the label is tied to it with `for`. Clicking the label activates the
 * button — `button` is a labelable element — so the reachable target is the whole label
 * line as well as the control, which is what a settings row does on every desktop OS.
 *
 * `role="switch"` with `aria-checked` is what makes the state announce rather than the
 * appearance, and the track is `aria-hidden` so it is not read as an unnamed image. The
 * control is disabled while a change is in flight and reports `aria-busy` at the same time,
 * because a toggle the OS has not confirmed yet must not accept a second press.
 */
export interface SwitchProps {
  readonly label: string;
  readonly description?: string;
  readonly checked: boolean;
  readonly disabled?: boolean;
  /** A change is in flight; the control refuses input until the authority answers. */
  readonly busy?: boolean;
  readonly onChange: (checked: boolean) => void;
  readonly className?: string;
}

export function Switch({
  label,
  description,
  checked,
  disabled = false,
  busy = false,
  onChange,
  className,
}: SwitchProps): ReactElement {
  const controlId = useId();

  return (
    <SettingsRow
      className={twMerge(
        clsx("transition-colors motion-reduce:transition-none hover:bg-surface-2", className),
      )}
      controlId={controlId}
      description={description}
      label={label}
    >
      <button
        aria-busy={busy ? true : undefined}
        aria-checked={checked}
        className={twMerge(
          clsx(
            "inline-flex h-7 w-9 shrink-0 items-center justify-center rounded-full",
            "focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus-ring",
            "disabled:pointer-events-none disabled:opacity-50",
          ),
        )}
        disabled={disabled || busy}
        id={controlId}
        onClick={() => {
          onChange(!checked);
        }}
        role="switch"
        type="button"
      >
        <span
          aria-hidden="true"
          className={twMerge(
            clsx(
              "relative block h-5 w-9 rounded-full border",
              "transition-colors motion-reduce:transition-none",
              checked ? "border-accent bg-accent" : "border-border-strong bg-surface-2",
            ),
          )}
        >
          <span
            className={twMerge(
              clsx(
                "absolute top-0.5 left-0.5 block size-4 rounded-full bg-surface-0",
                "transition-transform motion-reduce:transition-none",
                checked ? "translate-x-4" : "translate-x-0",
              ),
            )}
          />
        </span>
      </button>
    </SettingsRow>
  );
}
