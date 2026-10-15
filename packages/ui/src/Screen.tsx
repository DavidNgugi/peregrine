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
  /**
   * One line about what the screen is.
   *
   * `| undefined` for the reason `Section` gives: `exactOptionalPropertyTypes` is on, and a caller
   * that forwards its own optional value — or picks a subtitle only when it has something to say —
   * would otherwise have to build its props conditionally to express the same thing.
   */
  readonly subtitle?: string | undefined;
  readonly children: ReactNode;
  readonly className?: string | undefined;
}

export function Screen({ title, subtitle, children, className }: ScreenProps): ReactElement {
  const titleId = useId();

  return (
    <section
      aria-labelledby={titleId}
      // **`overflow-x-hidden` is load-bearing, and its absence was a bug.** `overflow-y-auto` is not
      // one axis: CSS computes the other from `visible` to `auto`, so a screen holding anything wide —
      // a rail's strip, a grid's spacer, a long table — became horizontally scrollable as a whole, and
      // panning it slid the page's own heading off behind the shell's sidebar. Horizontal scrolling
      // belongs to the thing that owns it (a rail's strip, a grid's window), never to a page.
      // `min-w-0` is the other half: without it a flex child defaults to `min-width: auto` and grows
      // to its content instead of to the box it was given, which is how a page's width escapes.
      className={twMerge(
        clsx("flex h-full min-w-0 flex-col gap-6 overflow-x-hidden overflow-y-auto px-6 py-4"),
        className,
      )}
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
