import { cva, type VariantProps } from "class-variance-authority";
import { clsx } from "clsx";
import type { ButtonHTMLAttributes, ReactElement, ReactNode } from "react";
import { twMerge } from "tailwind-merge";

const buttonVariants = cva(
  [
    "inline-flex items-center justify-center gap-2 rounded-md font-medium",
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
