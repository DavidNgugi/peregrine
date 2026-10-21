import { cva, type VariantProps } from "class-variance-authority";
import { clsx } from "clsx";
import type { ButtonHTMLAttributes, MouseEvent, ReactElement, ReactNode } from "react";
import { useState } from "react";
import { twMerge } from "./merge.js";

/**
 * A button's appearance, as a function of its variant and size.
 *
 * Exported to the kit rather than to the app, and for one caller: `Link` draws an anchor that has to
 * look like the button beside it when a caller asks for that, and a second copy of these classes in
 * `Link` would be a second answer to what a button looks like.
 */
export const buttonVariants = cva(
  [
    // `whitespace-nowrap` is not decoration: without it a two-word label wraps inside the button
    // when the row it sits in is wide enough for its description and not for the control, which
    // draws "Scan" over "now" and reads as a rendering fault rather than as a wrapped label.
    "inline-flex items-center justify-center gap-2 rounded-md font-medium whitespace-nowrap",
    "transition-colors motion-reduce:transition-none",
    "focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus-ring",
    "disabled:pointer-events-none disabled:opacity-50",
  ],
  {
    variants: {
      variant: {
        primary: "bg-accent text-text-on-accent hover:bg-accent-hover",
        secondary: "border border-border-subtle text-text-primary hover:bg-surface-2",
        ghost: "text-text-secondary hover:bg-surface-2 hover:text-text-primary",
        // Destructive, and deliberately not the accent: a red button and an accent button must not
        // look alike when one of them deletes something (AGENTS R22's confirmations use this).
        danger: "bg-state-danger text-text-on-accent hover:opacity-90",
      },
      size: {
        sm: "h-7 px-2 text-caption",
        md: "h-9 px-3 text-body",
      },
    },
    defaultVariants: { variant: "secondary", size: "md" },
  },
);

// `dangerouslySetInnerHTML` is removed from the prop surface so no call site can
// reach the sink the lint gate bans (SECURITY.md T8).
export type ButtonProps = Omit<
  ButtonHTMLAttributes<HTMLButtonElement>,
  "className" | "dangerouslySetInnerHTML"
> &
  VariantProps<typeof buttonVariants> & {
    readonly className?: string;
    readonly children: ReactNode;
    /**
     * The action is running. The button disables itself, sets `aria-busy`, shows a spinner,
     * and keeps the label visible: a control that turns into a bare spinner loses the one
     * thing a person needs while waiting, which is what they asked for.
     */
    readonly busy?: boolean;
    /**
     * The full label while `busy`. It should name the action, not repeat the verb:
     * "Saving the key", not "Saving". The idle label stays as it was.
     */
    readonly busyLabel?: string;
    /**
     * Completion from 0 to 100, drawn as a two-pixel bar along the button's foot. Only
     * meaningful with `busy`; a caller with phases passes the latest phase's figure.
     */
    readonly progress?: number;
  };

/** Whether a value is a promise a button can wait for. */
export function isThenable(value: unknown): value is Promise<unknown> {
  return (
    typeof value === "object" &&
    value !== null &&
    "then" in value &&
    typeof (value as { then?: unknown }).then === "function"
  );
}

/**
 * A button that consumes only semantic theme tokens.
 *
 * The busy state is deliberately inside the control rather than a screen-wide overlay: a
 * person keeps reading the page and can still use everything the action does not own. The
 * spinner and the progress bar animate only `transform` and `opacity`, so a running
 * action cannot take layout work away from scrolling or typing (DESIGN_SYSTEM §5).
 *
 * **An async click tracks itself.** When `onClick` returns a promise, the button shows the
 * spinner and disables itself until it settles, and re-renders nothing but its own label. A
 * screen that already owns a `busy` flag can pass it instead; the two are additive. This is
 * what keeps a long save or file copy off the screen's render path and gives every action a
 * visible running state without each feature inventing one.
 */
export function Button({
  variant,
  size,
  className,
  type = "button",
  children,
  busy = false,
  busyLabel,
  progress,
  disabled,
  onClick,
  ...rest
}: ButtonProps): ReactElement {
  const [running, setRunning] = useState(false);
  const busyNow = busy || running;
  const clamped =
    progress === undefined
      ? null
      : Math.min(100, Math.max(0, Number.isFinite(progress) ? progress : 0));

  const handleClick = (event: MouseEvent<HTMLButtonElement>): void => {
    const result: unknown = onClick?.(event);
    if (!isThenable(result)) {
      return;
    }
    setRunning(true);
    void result.then(
      () => {
        setRunning(false);
      },
      () => {
        setRunning(false);
      },
    );
  };

  return (
    <button
      aria-busy={busyNow ? true : undefined}
      className={twMerge(
        clsx(buttonVariants({ variant, size }), busyNow && "relative overflow-hidden", className),
      )}
      disabled={disabled === true || busyNow}
      onClick={handleClick}
      type={type}
      {...rest}
    >
      {busyNow ? (
        <span
          aria-hidden="true"
          className={clsx(
            "shrink-0 animate-spin rounded-full border-2 border-current border-t-transparent motion-reduce:animate-none",
            size === "sm" ? "size-3" : "size-3.5",
          )}
        />
      ) : null}
      {busyNow && busyLabel !== undefined ? busyLabel : children}
      {busyNow && clamped !== null ? (
        <span
          aria-hidden="true"
          className="pointer-events-none absolute inset-x-0 bottom-0 h-0.5 bg-current/20"
        >
          <span
            className="block h-full origin-left bg-current transition-transform duration-200 ease-out motion-reduce:transition-none"
            style={{ transform: `scaleX(${String(clamped / 100)})` }}
          />
        </span>
      ) : null}
    </button>
  );
}
