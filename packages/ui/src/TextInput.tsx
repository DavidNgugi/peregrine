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
  readonly className?: string;
}

export function TextInput({
  label,
  message,
  invalid = false,
  labelHidden = false,
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
          // than the browser's default outline.
          "focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-focus-ring",
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
