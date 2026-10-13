import { cva, type VariantProps } from "class-variance-authority";
import { clsx } from "clsx";
import type { ButtonHTMLAttributes, ReactElement, ReactNode } from "react";
import { twMerge } from "./merge.js";

const buttonVariants = cva(
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
  };

/** A button that consumes only semantic theme tokens. */
export function Button({
  variant,
  size,
  className,
  type = "button",
  children,
  ...rest
}: ButtonProps): ReactElement {
  return (
    <button
      type={type}
      className={twMerge(clsx(buttonVariants({ variant, size }), className))}
      {...rest}
    >
      {children}
    </button>
  );
}
