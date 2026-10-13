import { clsx } from "clsx";
import {
  useCallback,
  useEffect,
  useId,
  useRef,
  useState,
  type Key,
  type ReactElement,
  type ReactNode,
} from "react";
import { IconButton } from "./IconButton.js";
import { twMerge } from "./merge.js";

/**
 * A horizontal rail: one row of tiles that pages sideways, with its heading and its edges.
 *
 * **One tab stop, not one per tile.** §2.7 is explicit that a rail is a single stop: `Tab` moves to
 * the rail and away from it, and the arrows move *within* it. That is what makes a screen of ten rails
 * navigable — thirty `Tab` presses between two headings is the failure it prevents — and it is why the
 * component renders its items through a function rather than taking them as children: the caller builds
 * each tile, and the rail is what tells that tile which `tabIndex` to carry. Without that, a rail
 * could not move focus at all, because it cannot reach into an element it did not construct.
 *
 * **The edges are shown only when there is somewhere to go.** A chevron over an end someone has
 * already reached is a control that does nothing, and §6 requires every state designed: the two
 * chevrons appear and disappear with the overflow rather than being drawn faded. The rule that decides
 * it is a pure function, because the interesting cases are arithmetic at the boundary and a scroll
 * offset is not something a static render can produce (see `showsChevrons`).
 *
 * **What this does not do yet.** The roving model here covers the arrows, `Home`, and `End`;
 * Page-Up/Down, the geometrically-nearest-item move between rails, and Escape layering are PG-E7-19's
 * ("keyboard navigation model"), and the snapping and choreography of a scroll in flight are PG-E21-10's.
 * Both are named here rather than half-implemented: a rail that guessed at them would be a second
 * keyboard model to reconcile.
 */

export interface RailProps<T> {
  /** The rail's heading, which is also what names its region. */
  readonly title: string;
  readonly items: readonly T[];
  /** The stable domain id a tile is keyed by. `Key` rather than `string`, because a row id is often
   * a number and `String(id)` at every call site is a conversion with nothing to say. */
  readonly itemKey: (item: T) => Key;
  /**
   * Draws one tile. The `tabIndex` is the rail's, and it moves with the focus as the arrows do.
   */
  readonly renderItem: (item: T, tabIndex: number) => ReactNode;
  /** What the rail shows when it has nothing in it (PG-E7-08 requires one per rail). */
  readonly empty?: ReactNode;
  readonly className?: string;
}

/**
 * Where an arrow key takes the focus, or `null` for a key this rail does not own.
 *
 * `null` rather than the current index, so a caller can tell "this key is ours and the position is
 * unchanged" from "this key is not ours and something above us may want it" — Escape, for instance,
 * belongs to the overlay stack and must not be swallowed here.
 */
export function nextRailIndex(current: number, key: string, count: number): number | null {
  if (count <= 0) {
    return null;
  }
  // A rail whose items shrank under a held index — a filter changed while the focus was on tile nine —
  // must still answer something reachable rather than focus a child that is not there.
  const from = Math.min(Math.max(current, 0), count - 1);
  switch (key) {
    case "ArrowRight":
      return Math.min(from + 1, count - 1);
    case "ArrowLeft":
      return Math.max(from - 1, 0);
    case "Home":
      return 0;
    case "End":
      return count - 1;
    default:
      return null;
  }
}

/**
 * Which edge affordances an overflowing rail shows.
 *
 * The tolerance is a real boundary and not defensive coding: a scroll offset is fractional at any
 * device-pixel ratio other than 1, so a rail scrolled fully to its end reports `scrollWidth -
 * clientWidth - scrollLeft` of a third of a pixel, and a strict `> 0` would leave the forward chevron
 * lit on an end that cannot move. One pixel is below what a pointer can act on and above the residue.
 *
 * LTR only: in a right-to-left layout `scrollLeft` runs negative and both answers invert, which is
 * PG-E23's problem to solve once the layout is mirrored, not something to fake here.
 */
export function showsChevrons(
  scrollLeft: number,
  clientWidth: number,
  scrollWidth: number,
): { readonly start: boolean; readonly end: boolean } {
  if (scrollWidth - clientWidth <= 1) {
    return { start: false, end: false };
  }
  return { start: scrollLeft > 1, end: scrollWidth - clientWidth - scrollLeft > 1 };
}

/** The focusable element inside a rail item, which is the anchor its caller rendered. */
function focusableWithin(child: Element | undefined): HTMLElement | null {
  if (child === undefined) {
    return null;
  }
  if (child instanceof HTMLElement && child.matches("a, button, [tabindex]")) {
    return child;
  }
  return child.querySelector<HTMLElement>("a, button, [tabindex]");
}

export function Rail<T>({
  title,
  items,
  itemKey,
  renderItem,
  empty,
  className,
}: RailProps<T>): ReactElement {
  const titleId = useId();
  const scroller = useRef<HTMLUListElement | null>(null);
  const frame = useRef<number | null>(null);
  const [active, setActive] = useState(0);
  const [edges, setEdges] = useState({ start: false, end: false });

  const measure = useCallback(() => {
    const element = scroller.current;
    if (element === null) {
      return;
    }
    setEdges(showsChevrons(element.scrollLeft, element.clientWidth, element.scrollWidth));
  }, []);

  useEffect(() => {
    const element = scroller.current;
    if (element === null) {
      return;
    }
    // Coalesced to one measurement per frame: the listener is passive and never writes to a style, so
    // it cannot make the scroll it is watching jank, and a burst of events costs one re-render (§2.2).
    const schedule = (): void => {
      if (frame.current !== null) {
        return;
      }
      frame.current = requestAnimationFrame(() => {
        frame.current = null;
        measure();
      });
    };
    measure();
    element.addEventListener("scroll", schedule, { passive: true });
    const observer = new ResizeObserver(schedule);
    observer.observe(element);
    return () => {
      element.removeEventListener("scroll", schedule);
      observer.disconnect();
      if (frame.current !== null) {
        cancelAnimationFrame(frame.current);
        frame.current = null;
      }
    };
  }, [measure]);

  const page = (direction: 1 | -1): void => {
    const element = scroller.current;
    if (element === null) {
      return;
    }
    const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    element.scrollBy({
      behavior: reduced ? "auto" : "smooth",
      left: direction * element.clientWidth * 0.9,
    });
  };

  const moveFocus = (next: number): void => {
    setActive(next);
    // Focus after the render that gives the new tile its tab stop; the lookup is by position because
    // the caller owns what each item is, not this component.
    requestAnimationFrame(() => {
      focusableWithin(scroller.current?.children[next])?.focus();
    });
  };

  return (
    <section aria-labelledby={titleId} className={twMerge(clsx("flex flex-col gap-2"), className)}>
      <h2 className="text-title-2 text-text-primary" id={titleId}>
        {title}
      </h2>
      {items.length === 0 ? (
        (empty ?? null)
      ) : (
        <div className="group/rail relative min-w-0">
          {/* `min-w-0` above is load-bearing rather than cosmetic. A flex child defaults to
              `min-width: auto`, so this wrapper grows to the scroller's *content* width instead of the
              space it was given; the chevrons are positioned against this box, so they anchored
              off-screen at the far right of the overflowing content and the screen clipped where the
              rail should have scrolled. Found by looking at the running app against a real provider's
              rails: ten tiles wide and not one edge affordance visible. */}
          {/* `py-2` is not spacing: the tiles' focus ring is offset 2 px outside their frame, and a
              scroller that clips on both axes would cut it off on the first and last tile. There is
              deliberately no `snap-*` here: snapping moves the resting offset off zero, which is what
              decides whether the back chevron is lit, and the choreography of a scroll in flight is
              PG-E21-10's rather than a class guessed at now. */}
          <ul
            className="flex gap-3 overflow-x-auto px-0.5 py-2"
            onKeyDown={(event) => {
              const next = nextRailIndex(active, event.key, items.length);
              if (next === null) {
                return;
              }
              event.preventDefault();
              moveFocus(next);
            }}
            ref={scroller}
          >
            {items.map((item, index) => (
              <li className="shrink-0" key={itemKey(item)}>
                {renderItem(item, index === active ? 0 : -1)}
              </li>
            ))}
          </ul>
          {edges.start ? (
            <span className="absolute top-1/2 left-0 -translate-y-1/2">
              <IconButton
                icon="chevron-left"
                label={`Scroll ${title} back`}
                onClick={() => {
                  page(-1);
                }}
                variant="secondary"
              />
            </span>
          ) : null}
          {edges.end ? (
            <span className="absolute top-1/2 right-0 -translate-y-1/2">
              <IconButton
                icon="chevron-right"
                label={`Scroll ${title} on`}
                onClick={() => {
                  page(1);
                }}
                variant="secondary"
              />
            </span>
          ) : null}
        </div>
      )}
    </section>
  );
}
