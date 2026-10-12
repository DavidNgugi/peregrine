import { clsx } from "clsx";
import { useId } from "react";
import type { ReactElement, ReactNode } from "react";
import { twMerge } from "./merge.js";

/**
 * A screen: the labelled region the shell swaps, and the one heading a page has.
 *
 * It owns the padding, the scrolling, the `<h1>` and the id that names the region, so every
 * screen in the app starts at the same inset and announces the same landmark. The subtitle
 * sits on the heading's baseline on the trailing edge, which is where a screen that has one
 * line to say about itself puts it.
 *
 * The scroll lives here rather than on the document: `apps/desktop/src/styles.css` keeps the
 * document from scrolling at all, so a screen longer than the window scrolls inside its own
 * region and the chrome, the sidebar, and the player bar stay put (§6 rule 6).
 */
export interface ScreenProps {
  readonly title: string;
  /** One line about what the screen is. */
  readonly subtitle?: string;
  readonly children: ReactNode;
  readonly className?: string;
}

export function Screen({ title, subtitle, children, className }: ScreenProps): ReactElement {
  const titleId = useId();

  return (
    <section
      aria-labelledby={titleId}
      className={twMerge(clsx("flex h-full flex-col gap-6 overflow-y-auto px-6 py-4"), className)}
    >
      <header className="flex items-baseline justify-between gap-4">
        <h1 className="text-title-1 text-text-primary" id={titleId}>
          {title}
        </h1>
        {subtitle === undefined ? null : (
          <p className="text-caption text-text-tertiary">{subtitle}</p>
        )}
      </header>
      {children}
    </section>
  );
}
