import { clsx } from "clsx";
import type { InputHTMLAttributes, ReactElement } from "react";
import { useId } from "react";
import { twMerge } from "./merge.js";

/**
 * A single line of text, with the label and the message that belong to it.
 *
 * The label is a prop rather than a child for the reason the rest of the kit gives: a caller who
 * supplies their own `<label>` will eventually forget the `htmlFor`, and an input whose label is only
 * visually associated is an input a screen reader announces as blank. The id is minted here and used
 * for both the label and, when there is one, the message.
 *
 * The message is wired to `aria-describedby` and carries `role="alert"` when it is an error: a
 * validation failure that appears silently beside a field is a failure a screen reader user never
 * hears (§9). It is one prop rather than two because an input has one thing to say about itself at a
 * time.
 */
export interface TextInputProps extends Omit<
  InputHTMLAttributes<HTMLInputElement>,
  "className" | "id" | "children"
> {
  /** The visible label, which is also the accessible name. */
  readonly label: string;
  /** A sentence under the field: help, or the reason the value was refused. */
  readonly message?: string;
  /** Whether the message is a failure, which announces it and colours it. */
  readonly invalid?: boolean;
  /** Hides the label visually while keeping it for assistive technology. */
  readonly labelHidden?: boolean;
  /**
   * Which surface draws the focus affordance.
   *
   * `"field"` is the default and draws it on the input itself — the token ring, not the browser's
   * default outline. `"none"` says the *caller* owns it, for a field restyled into part of a composite
   * control: Discover's search box is an input inside a pill, and with the default the input kept its
   * own rounded outline inside that pill, so focusing it drew **two concentric pills**. A caller that
   * passes `"none"` must put the ring on the element a reader sees as the field.
   */
  readonly focusRing?: "field" | "none";
  readonly className?: string;
}

export function TextInput({
  label,
  message,
  invalid = false,
  labelHidden = false,
  focusRing = "field",
  className,
  ...rest
}: TextInputProps): ReactElement {
  const id = useId();
  const messageId = `${id}-message`;
  return (
    <div className={twMerge(clsx("flex flex-col gap-1"), className)}>
      <label
        className={clsx("text-caption text-text-secondary", labelHidden && "sr-only")}
        htmlFor={id}
      >
        {label}
      </label>
      <input
        aria-describedby={message === undefined ? undefined : messageId}
        aria-invalid={invalid ? true : undefined}
        className={clsx(
          "h-9 rounded-sm border bg-surface-2 px-2 text-body text-text-primary",
          // The focus ring is the token, so it matches every other control on the platform rather
          // than the browser's default outline — and it is *only* drawn when this component owns the
          // affordance, because a caller that has built a larger control around the field draws it
          // there instead.
          focusRing === "field"
            ? "focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-focus-ring"
            : "focus-visible:outline-none",
          invalid ? "border-state-danger" : "border-border-subtle",
        )}
        id={id}
        {...rest}
      />
      {message === undefined ? null : (
        <p
          className={clsx("text-caption", invalid ? "text-state-danger" : "text-text-tertiary")}
          id={messageId}
          role={invalid ? "alert" : undefined}
        >
          {message}
        </p>
      )}
    </div>
  );
}
