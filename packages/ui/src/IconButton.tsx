import { cva, type VariantProps } from "class-variance-authority";
import { clsx } from "clsx";
import type { ButtonHTMLAttributes, MouseEvent, ReactElement } from "react";
import { useState } from "react";
import { isThenable } from "./Button.js";
import { twMerge } from "./merge.js";
import { Icon, type IconName } from "./Icon.js";

const iconButtonVariants = cva(
  [
    "inline-flex items-center justify-center rounded-md",
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
      },
      size: {
        // 28 px, 32 px, and 44 px, all above the pointer minimum of §4. The icon scales with
        // the box rather than being chosen separately, so a mark can never end up in a target
        // smaller than it, and the 44 px size is the one a screen's own primary control wants.
        sm: "size-7",
        md: "size-8",
        lg: "size-11",
      },
    },
    defaultVariants: { variant: "ghost", size: "md" },
  },
);

// `aria-label` is removed from the prop surface so the required `label` below is the only
// way to name the control, and `dangerouslySetInnerHTML` for the same reason `Button`
// removes it (SECURITY.md T8).
export type IconButtonProps = Omit<
  ButtonHTMLAttributes<HTMLButtonElement>,
  "aria-label" | "children" | "className" | "dangerouslySetInnerHTML"
> &
  VariantProps<typeof iconButtonVariants> & {
    /** The accessible name. An icon on its own has none, so this is not optional (§9). */
    readonly label: string;
    readonly icon: IconName;
    readonly className?: string;
    /** The action is running; the icon becomes a spinner and the control is disabled. */
    readonly busy?: boolean;
  };

/**
 * A button that draws one icon and no text.
 *
 * The label is what a screen reader announces and the default native tooltip, and it is a
 * required prop rather than an optional one because an icon-only control without it is
 * unusable by anyone not looking at it. A caller may pass a shorter `title`; the accessible
 * name stays the full label. The richer tooltip primitive is PG-E21-03's.
 */
export function IconButton({
  variant,
  size,
  className,
  icon,
  label,
  busy = false,
  disabled,
  onClick,
  title,
  type = "button",
  ...rest
}: IconButtonProps): ReactElement {
  const [running, setRunning] = useState(false);
  const busyNow = busy || running;

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
      aria-label={label}
      className={twMerge(clsx(iconButtonVariants({ variant, size }), className))}
      disabled={disabled === true || busyNow}
      onClick={handleClick}
      title={title ?? label}
      type={type}
      {...rest}
    >
      {busyNow ? (
        <span
          aria-hidden="true"
          className={clsx(
            "animate-spin rounded-full border-2 border-current border-t-transparent motion-reduce:animate-none",
            size === "sm" ? "size-3.5" : size === "lg" ? "size-5" : "size-4",
          )}
        />
      ) : (
        <Icon name={icon} size={size === "sm" ? 16 : size === "lg" ? 24 : 20} />
      )}
    </button>
  );
}
