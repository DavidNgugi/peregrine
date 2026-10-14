import { clsx } from "clsx";
import type { AnchorHTMLAttributes, MouseEvent, ReactElement, ReactNode } from "react";
import { buttonVariants } from "./Button.js";
import { twMerge } from "./merge.js";

/**
 * A link, which is a button that says where it goes.
 *
 * The difference is not cosmetic. A link is announced as a link, it can be opened in a new window or
 * copied by a context menu, it carries the destination in the markup rather than in a closure, and it
 * is what a keyboard user expects to move between with `Tab` and open with `Enter`. A button that
 * happens to navigate has none of that, which is what the library screen had until this existed.
 *
 * **The kit holds no router.** A link renders an `href` and, when the caller gives one, a handler that
 * takes the click instead of letting the document follow it — which in a webview is the difference
 * between moving to the next screen and reloading the application. What a route *is* belongs to the
 * app (`app/routes.ts`); this component only knows that a path is a string.
 *
 * The handler is called with the click's own event so a caller can decide for itself whether a
 * modified click (a new tab, a download) should be left alone: the kit prevents the default only
 * because the caller asked it to, and never swallows a click the caller did not ask about.
 */
export interface LinkProps extends Omit<
  AnchorHTMLAttributes<HTMLAnchorElement>,
  "className" | "href" | "children"
> {
  /** Where it goes, which is what the markup carries and what a context menu offers. */
  readonly href: string;
  readonly children: ReactNode;
  /**
   * Takes the click instead of letting the document navigate.
   *
   * Given, the default is prevented and this runs. Absent, the anchor behaves as it is written — which
   * is what an external link wants, and what a test wants when it renders the markup alone.
   */
  readonly onNavigate?: () => void;
  /**
   * Draws the anchor as a button of this variant, for a link that stands beside one.
   *
   * The element does not change and must not: Discover's hero has a Play *button* and a Details
   * *link* side by side, and the link has to look like the button without becoming one — the point of
   * an anchor is that the destination is in the markup, that a context menu can offer it, and that
   * `Enter` opens it, which a `<button>` with an `onClick` has none of. The classes come from
   * `buttonVariants`, so the two cannot drift.
   */
  readonly variant?: "primary" | "secondary" | "ghost" | "danger";
  readonly size?: "sm" | "md";
  readonly className?: string;
}

/**
 * Whether a click is the caller's to take.
 *
 * A pure function rather than a closure inside the component, for the reason the overlay stack is a
 * module: `renderToStaticMarkup` produces a string and no events, so a decision that lived in the
 * component's props could not be exercised at all. Here it is six booleans and a call, and the tests
 * drive it directly.
 *
 * A click with a modifier is a request for somewhere else — a new window, a download, a search — and
 * the kit follows the platform's convention rather than its own. A click another handler already
 * prevented is that handler's decision, not the link's to overrule.
 */
export function shouldNavigate(
  event: Pick<
    MouseEvent<HTMLAnchorElement>,
    "altKey" | "ctrlKey" | "defaultPrevented" | "metaKey" | "shiftKey"
  >,
  hasNavigation: boolean,
): boolean {
  return (
    hasNavigation &&
    !event.defaultPrevented &&
    !event.metaKey &&
    !event.ctrlKey &&
    !event.shiftKey &&
    !event.altKey
  );
}

export function Link({
  href,
  children,
  onNavigate,
  variant,
  size,
  className,
  onClick,
  ...rest
}: LinkProps): ReactElement {
  const handle = (event: MouseEvent<HTMLAnchorElement>): void => {
    onClick?.(event);
    if (shouldNavigate(event, onNavigate !== undefined)) {
      event.preventDefault();
      onNavigate?.();
    }
  };

  return (
    <a
      className={twMerge(
        clsx(
          variant === undefined
            ? "rounded-sm text-text-primary underline-offset-2 hover:underline focus-visible:outline-offset-1"
            : buttonVariants({ variant, size }),
          "focus-visible:outline-2 focus-visible:outline-focus-ring",
        ),
        className,
      )}
      href={href}
      onClick={handle}
      {...rest}
    >
      {children}
    </a>
  );
}
