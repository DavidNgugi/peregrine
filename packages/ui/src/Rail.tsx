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
import { revealRoom } from "./hover-preview.js";
import { IconButton } from "./IconButton.js";
import { twMerge } from "./merge.js";
import { Skeleton } from "./Skeleton.js";
import { CARD_ASPECT, CARD_WIDTHS, type CardSize } from "./TitleCard.js";

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
 *
 * **The scroller clips, so the rail leaves the hover ladder room.** `overflow-x: auto` is not one
 * axis: CSS computes `overflow-y` to `auto` as well when the other axis is not `visible`, so the
 * scroller is a clip box on both axes and a revealed tile is cut by its own rail — measured in the
 * running gallery, where the tile's ramp reached 1.5 px past the tile and 42 px of it fell outside
 * the padding box. The room is padding, sized by `revealRoom` from the largest tile the kit draws;
 * the negative margins of the same size put the row back where it was, so nothing below a rail moves
 * and the room is only ever the clip box. The tile's own lift over its neighbours is
 * `TitleCard`'s (`REVEAL_LIFT_Z`), because painting order is the card's business and not the rail's.
 */

/**
 * The inset the row's tiles rest at, which is what a screen's rhythm is built on: the tiles sit this
 * far from the box the rail occupies.
 *
 * It is a named constant rather than the `px-0.5 py-2` it replaced because the room below is written
 * around it: the scroller's padding is the room, and its negative margin gives back everything except
 * this, so enlarging the clip box moves nothing — not the row, not the heading above it, not the
 * rail below.
 */
export const RAIL_INSET = { x: 2, y: 8 } as const;

/**
 * The room the scroller leaves an enlarged tile, from the largest tile the kit draws.
 *
 * `xl` rather than the tile this rail actually holds, because a rail renders its items through the
 * caller's function and cannot ask what size they are — so the room is the worst case, and a rail of
 * `sm` tiles pays a few pixels it does not need. The height is §4's ratio applied to that width.
 */
const ROOM = revealRoom(CARD_WIDTHS.xl, CARD_WIDTHS.xl / CARD_ASPECT);

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
  /**
   * The tile size the rail draws placeholders at while its items are on the way, or `undefined` for
   * a rail that is not loading (`PG-E7-16`'s loading state).
   *
   * A size rather than a node, because the row's geometry is this component's: the placeholders have
   * to sit on the same inset, the same gap, and the same clip box as tiles would, or the arrival of
   * the real content moves the heading above and the rail below — which is the whole point of a
   * skeleton (§11 rule 2). A placeholder is decoration and carries no tab stop: a loading rail is not
   * a keyboard stop, and there is nothing in it to move focus to.
   *
   * It applies only when the rail has no items: a rail that is *revalidating* keeps drawing what it
   * has, because replacing readable content with placeholders is a worse answer than the stale one.
   */
  readonly skeleton?: CardSize;
  readonly className?: string;
}

/**
 * How many placeholders a loading rail draws.
 *
 * A screenful at every tile size the kit ships (a rail clips what does not fit), and constant rather
 * than derived from a measurement because a count that changed with the window would re-render the
 * row on every resize for no gain: nothing here is real content.
 */
export const SKELETON_TILES = 8;

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

/**
 * The placeholders a loading rail draws, on the row's own inset, gap, and clip box.
 *
 * A `div` rather than the `ul` its items live in: there are no items, so there is no list, and a list
 * of eight things a reader cannot act on is noise. It is `aria-hidden` for the same reason — the
 * state is announced in words by whatever put the rail into it — while `aria-busy` on the rail's own
 * region is what tells assistive technology the section is still filling.
 */
function SkeletonRow({ size }: { readonly size: CardSize }): ReactElement {
  const width = CARD_WIDTHS[size];
  return (
    <div
      aria-hidden="true"
      className="flex gap-3 overflow-hidden"
      style={{
        margin: `${String(RAIL_INSET.y - ROOM.top)}px ${String(RAIL_INSET.x - ROOM.sides)}px ${String(RAIL_INSET.y - ROOM.bottom)}px`,
        padding: `${String(ROOM.top)}px ${String(ROOM.sides)}px ${String(ROOM.bottom)}px`,
      }}
    >
      {Array.from({ length: SKELETON_TILES }, (_, index) => (
        // The index is the identity here and that is not the rule about keys being domain ids: a
        // fixed count of placeholders never reorders, and a placeholder has no domain to be identified
        // by. The width and the ratio are the tile's, from the constants `TitleCard` draws itself
        // with, because a placeholder that guessed at either would move the row when the tile
        // arrived — which is the one thing a skeleton exists to prevent.
        <Skeleton
          className="shrink-0"
          key={index}
          style={{ aspectRatio: String(CARD_ASPECT), width }}
        />
      ))}
    </div>
  );
}

export function Rail<T>({
  title,
  items,
  itemKey,
  renderItem,
  empty,
  skeleton,
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
    <section
      aria-busy={skeleton !== undefined && items.length === 0}
      aria-labelledby={titleId}
      // **The rail clips its own horizontal overhang, and that is a bug fix rather than tidiness.**
      // The strip's deliberate overhang — the room the hover ladder and the edge chevrons need — sits
      // *outside* the scroller's clip box, so with `overflow: visible` here it leaked to whatever
      // ancestor happened to be a scroll container. A page whose own `overflow-y: auto` computes
      // `overflow-x: auto` then grew a horizontal scrollbar and the whole view panned sideways, which
      // is exactly what a reader reported. `clip` rather than `hidden` on purpose: it contains the
      // overhang *without* becoming a scroll container, so it cannot swallow the vertical room the
      // enlarged card needs and cannot add a second horizontal scrollbar of its own.
      className={twMerge(clsx("flex flex-col gap-2 overflow-x-clip"), className)}
      style={{ overflowClipMargin: `${String(ROOM.sides)}px` }}
    >
      <h2 className="text-title-2 text-text-primary" id={titleId}>
        {title}
      </h2>
      {items.length === 0 && skeleton !== undefined ? (
        <SkeletonRow size={skeleton} />
      ) : items.length === 0 ? (
        (empty ?? null)
      ) : (
        <div className="group/rail relative min-w-0">
          {/* `min-w-0` above is load-bearing rather than cosmetic. A flex child defaults to
              `min-width: auto`, so this wrapper grows to the scroller's *content* width instead of the
              space it was given; the chevrons are positioned against this box, so they anchored
              off-screen at the far right of the overflowing content and the screen clipped where the
              rail should have scrolled. Found by looking at the running app against a real provider's
              rails: ten tiles wide and not one edge affordance visible. */}
          {/* The padding is not spacing and the margins are not a trick to hide it: together they
              enlarge the scroller's *clip box* — which is the padding box, and which `overflow-x:
              auto` also makes the vertical clip — while leaving the tiles exactly where they were,
              so the chevrons' arithmetic, the row's own inset, and every heading on the screen are
              unchanged. The values come from `revealRoom` at the largest tile: a scaled tile, its
              focus ring, and the shadow ramp that is the lift, which the scroller cut to a sliver of
              its 32 px blur. There is deliberately no `snap-*` here: snapping moves the resting
              offset off zero, which is what decides whether the back chevron is lit, and the
              choreography of a scroll in flight is PG-E21-10's rather than a class guessed at now. */}
          <ul
            className="no-scrollbar flex gap-3 overflow-x-auto"
            onKeyDown={(event) => {
              const next = nextRailIndex(active, event.key, items.length);
              if (next === null) {
                return;
              }
              event.preventDefault();
              moveFocus(next);
            }}
            ref={scroller}
            style={{
              margin: `${String(RAIL_INSET.y - ROOM.top)}px ${String(RAIL_INSET.x - ROOM.sides)}px ${String(RAIL_INSET.y - ROOM.bottom)}px`,
              padding: `${String(ROOM.top)}px ${String(ROOM.sides)}px ${String(ROOM.bottom)}px`,
            }}
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
