import { clsx } from "clsx";
import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type Key,
  type ReactElement,
  type ReactNode,
} from "react";
import { twMerge } from "./merge.js";

/**
 * A virtualized grid of fixed-geometry tiles.
 *
 * **The geometry is fixed, so the window is arithmetic.** §4 gives a poster one aspect ratio and a
 * small set of widths, and this component takes the tile's width and its row height as numbers rather
 * than measuring anything. That is what makes it possible to know which tiles are on screen without
 * laying anything out: `gridWindow` is a pure function of four numbers, it is where every boundary case
 * lives, and the component below is the part that cannot be tested without a DOM. A grid that measured
 * its children instead would need a library, a layout pass, and a test harness this repository does not
 * have (PG-E21-03's named gap) to answer the same question.
 *
 * **Five thousand tiles are not five thousand nodes.** `PG-E7-06`'s acceptance is 5 000 items at 60 fps,
 * and the reason a naive grid cannot hold it is node count, not styling: the component renders one row
 * of overscan beyond each visible edge and a single spacer carrying the full height, so the DOM holds
 * tens of tiles whatever the catalogue holds. The frame rate itself is *not* asserted here — the
 * frame-time gate is PG-E7-20's and this repository has no bench harness yet — so this task claims the
 * bounded node count and leaves the measurement to the gate that owns it.
 *
 * **It scrolls itself, and the caller must bound its height.** Virtualization needs to know which
 * element scrolls, and a grid inside a page that scrolls would have to correlate two offsets and the
 * page's own layout. The grid is therefore its own scroller, filling a parent the caller has sized
 * (`h-80`, `flex-1 min-h-0`, or a pane with a protocol height). This is a real precondition rather than
 * a note: a scroller with no height constraint grows to its content, so the "viewport" becomes the whole
 * grid and every tile is rendered — the failure the component exists to prevent, and the one the
 * gallery's first version had until a measurement in the browser showed 5 000 list items instead of 24.
 */

/** What a windowed grid draws: a contiguous slice of the items, and where that slice sits. */
export interface GridWindow {
  /** First item drawn, inclusive. */
  readonly first: number;
  /** Last item drawn, exclusive. */
  readonly last: number;
  /** How many rows the slice spans, which is what positions it. */
  readonly rows: number;
  /** The slice's distance from the top of the full grid, in pixels. */
  readonly offset: number;
}

export interface GridWindowInput {
  readonly itemCount: number;
  readonly columns: number;
  /** A row's height *including* the gap below it, so rows tile without a second measurement. */
  readonly rowHeight: number;
  readonly viewportHeight: number;
  readonly scrollTop: number;
  /** Rows drawn beyond each edge, so a fast scroll does not meet an empty strip. */
  readonly overscanRows: number;
}

/**
 * Which items a grid at this offset draws.
 *
 * Every clamp here is a boundary that a scrolling grid reaches: the top (a negative or fractional
 * `scrollTop` from a rubber-band overscroll), the bottom (a viewport taller than the content, and an
 * offset past the end after the items shrank under it), and a configuration of zero — a container that
 * has not been laid out yet reports `clientWidth` and `clientHeight` of 0, which must draw the first
 * rows rather than nothing, or the grid would be blank until the first observer callback.
 */
export function gridWindow({
  itemCount,
  columns,
  rowHeight,
  viewportHeight,
  scrollTop,
  overscanRows,
}: GridWindowInput): GridWindow {
  if (itemCount <= 0 || columns <= 0 || rowHeight <= 0) {
    return { first: 0, last: 0, offset: 0, rows: 0 };
  }
  const totalRows = Math.ceil(itemCount / columns);
  const offset = Math.max(scrollTop, 0);
  const visibleFirst = Math.floor(offset / rowHeight);
  // `+ 1` because a viewport rarely ends on a row boundary, and the row it ends inside is visible.
  const visibleLast = Math.floor((offset + Math.max(viewportHeight, 0)) / rowHeight) + 1;
  const overscan = Math.max(overscanRows, 0);
  // The top clamp is against the *last* row and not against zero alone: an offset past the end — the
  // state a filter leaves behind when it cuts a thousand results to forty — would otherwise put
  // `firstRow` beyond `totalRows`, and `lastRow - firstRow` would go negative: a slice with no items
  // in it and a window translated past the bottom of its own spacer. Clamping to the last row draws
  // the row the offset is nearest instead, which is what the eye expects after a list shrinks.
  const firstRow = Math.min(Math.max(visibleFirst - overscan, 0), Math.max(totalRows - 1, 0));
  const lastRow = Math.min(Math.max(visibleLast + overscan, firstRow + 1), totalRows);
  const rows = lastRow - firstRow;
  return {
    first: firstRow * columns,
    last: Math.min(lastRow * columns, itemCount),
    offset: firstRow * rowHeight,
    rows,
  };
}

/**
 * How many tiles fit across a container.
 *
 * `gap` is the space *between* tiles, so `columns` tiles take `columns * tileWidth + (columns - 1) *
 * gap`: the `+ gap` in the arithmetic below turns that into the division, and the result is that a
 * container one pixel too narrow for another tile keeps the column count rather than overflowing it.
 * A container narrower than one tile still gets one column, because a grid of zero columns is a grid
 * that shows nothing.
 */
export function gridColumns(width: number, tileWidth: number, gap: number): number {
  if (!(tileWidth > 0) || !(gap >= 0) || !(width > 0)) {
    return 1;
  }
  return Math.max(1, Math.floor((width + gap) / (tileWidth + gap)));
}

/**
 * Whether the drawn slice has come within `threshold` items of the end of `itemCount`.
 *
 * **A pure function for the reason [`gridWindow`] is one**: the grid's effects cannot be tested in
 * this repository — the harness renders to static markup in a Node environment and has no DOM — so
 * every decision the component makes is extracted to arithmetic that a test can call. The boundary
 * cases are the ones a scroller reaches: an empty list never reaches the end (there is nothing to
 * load), a slice already at the end always has, and a threshold larger than the list means the first
 * render is already near enough.
 */
export function gridEndReached(itemCount: number, drawn: GridWindow, threshold: number): boolean {
  return itemCount > 0 && drawn.last >= itemCount - Math.max(threshold, 0);
}

export interface GridProps<T> {
  /** The region's accessible name, which is what a screen reader announces on entry. */
  readonly label: string;
  readonly items: readonly T[];
  readonly itemKey: (item: T) => Key;
  readonly renderItem: (item: T) => ReactNode;
  /** The tile's width, in the same units as the tile itself: §4's ladder. */
  readonly tileWidth: number;
  /** A row's height including the gap below it. */
  readonly rowHeight: number;
  /** The gap between tiles, which is also the gap between rows' tiles. */
  readonly gap?: number;
  /**
   * Pins the column count instead of deriving it from the container.
   *
   * The derived count is what makes the grid reflow with the window; a caller that pins it is either
   * testing the layout or honouring the density setting PG-E7-18 owns.
   */
  readonly columns?: number;
  readonly overscanRows?: number;
  /**
   * Called when the drawn slice comes within [`GridProps::endThreshold`] items of the end.
   *
   * **The grid owns the scroll position, so it is the only thing that can say this.** A caller cannot
   * observe this scroller from outside it, and a sentinel element placed after the tiles would be
   * virtualized away with them. The callback takes no arguments — what to fetch, and whether there is
   * anything left to fetch, is the caller's business — and it fires on every render where the
   * condition holds, so a caller must coalesce for itself (the browse read ignores a request while one
   * is in flight, which is what stops a slow page from being asked for twice).
   */
  readonly onEndReached?: () => void;
  /** How near the end, in items, the drawn slice must come to call [`GridProps::onEndReached`]. */
  readonly endThreshold?: number;
  /**
   * A value that changes when the caller *replaces* the list rather than appending to it.
   *
   * **A replaced list starts at the top.** The scroller's own position survives a change of items —
   * it is the same element — so a query whose previous answer was scrolled a thousand pixels down
   * would draw its first page from a thousand pixels down, or (because the browser clamps a scroll
   * offset past the end while this component's copy of it is stale) from the end of a list nobody has
   * looked at yet. Changing this value scrolls back to the top, which is the only position a new
   * answer makes sense from. An append must *not* change it.
   */
  readonly resetKey?: string | number;
  /**
   * Whether a page after the drawn items is on its way.
   *
   * **A grid with more to fetch has to say so**, or a reader who has reached the end of what is
   * loaded cannot tell "there is nothing more" from "the next page is still coming"
   * (`DESIGN_SYSTEM §11` rule 2). The placeholders are laid out *inside* the grid's own flow, below
   * the last real tile, so they sit exactly where the arriving tiles will — which is what keeps the
   * geometry still when they arrive, the same reason [`GridProps::skeleton`](crate::Rail) reserves a
   * row.
   */
  readonly loading?: boolean;
  /**
   * The tile drawn for a cell that is still on its way. Without one, `loading` reserves the space
   * and draws nothing in it.
   */
  readonly placeholder?: ReactNode;
  /**
   * How many rows of placeholders to reserve while loading.
   *
   * One is the default because a page is usually a couple of rows and one is enough to say "more is
   * coming"; a caller that knows its page size can reserve exactly that many.
   */
  readonly placeholderRows?: number;
  readonly className?: string;
}

export function Grid<T>({
  label,
  items,
  itemKey,
  renderItem,
  tileWidth,
  rowHeight,
  gap = 12,
  columns: pinnedColumns,
  overscanRows = 1,
  onEndReached,
  endThreshold = 8,
  resetKey,
  loading = false,
  placeholder,
  placeholderRows = 1,
  className,
}: GridProps<T>): ReactElement {
  const scroller = useRef<HTMLDivElement | null>(null);
  const frame = useRef<number | null>(null);
  const settle = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [viewport, setViewport] = useState({ height: 0, width: 0 });
  const [scrollTop, setScrollTop] = useState(0);
  const [scrolling, setScrolling] = useState(false);

  const measure = useCallback(() => {
    const element = scroller.current;
    if (element === null) {
      return;
    }
    setViewport({ height: element.clientHeight, width: element.clientWidth });
  }, []);

  useEffect(() => {
    const element = scroller.current;
    if (element === null) {
      return;
    }
    // Passive and coalesced to one state update per frame (§2.2): a scroll handler that writes to
    // layout on every event is the way a virtualized grid ends up slower than the naive one.
    const schedule = (): void => {
      if (frame.current !== null) {
        return;
      }
      frame.current = requestAnimationFrame(() => {
        frame.current = null;
        setScrollTop(element.scrollTop);
        // `will-change` is applied while the grid is moving and removed once it settles (§5 rule 3):
        // left on, it is a GPU layer per grid that never goes away.
        setScrolling(true);
        if (settle.current !== null) {
          clearTimeout(settle.current);
        }
        settle.current = setTimeout(() => {
          setScrolling(false);
        }, 150);
      });
    };
    measure();
    element.addEventListener("scroll", schedule, { passive: true });
    const observer = new ResizeObserver(() => {
      measure();
    });
    observer.observe(element);
    return () => {
      element.removeEventListener("scroll", schedule);
      observer.disconnect();
      if (frame.current !== null) {
        cancelAnimationFrame(frame.current);
        frame.current = null;
      }
      if (settle.current !== null) {
        clearTimeout(settle.current);
        settle.current = null;
      }
    };
  }, [measure]);

  const columns = pinnedColumns ?? gridColumns(viewport.width, tileWidth, gap);
  // The cells the grid lays out: the items, plus the rows reserved for the page on its way. The
  // reservation is what keeps the scrollbar's height honest while the next page is in the air, and
  // what puts the placeholders where the arriving tiles are going to be.
  const reserved = loading ? Math.max(placeholderRows, 0) * columns : 0;
  const cells = items.length + reserved;
  const window_ = gridWindow({
    columns,
    itemCount: cells,
    overscanRows,
    rowHeight,
    scrollTop,
    viewportHeight: viewport.height,
  });
  const totalRows = cells === 0 ? 0 : Math.ceil(cells / columns);

  // The callback is read through a ref so an inline arrow from the caller does not make the effect
  // below depend on a new function on every render and fire a request on all of them.
  const endReachedRef = useRef(onEndReached);
  endReachedRef.current = onEndReached;
  // Reaching within `endThreshold` of the end of the **real items**, not of the reserved cells: a
  // reservation is the grid saying "more is coming", and counting it as content would ask for a page
  // that is already in the air.
  const endReached = gridEndReached(items.length, window_, endThreshold);
  // **The condition is re-evaluated when the content changes, not only when it flips.** An effect
  // keyed on the boolean alone fires on `false → true` and nothing else, so a grid that was already
  // at the end when a page landed never asked again — the reader had to scroll away and back to
  // re-arm it, and a list whose first page did not fill the viewport stopped after one page. What
  // this effect answers is "is there more to ask for *now*", and the caller's own in-flight guard is
  // what makes re-asking every time the answer changes a no-op rather than a duplicate request.
  //
  // `items.length` and `loading` are the two arrivals that can change the answer without a scroll:
  // more items (still at the end ⇒ keep filling) and the end of a request (the guard has lifted).
  useEffect(() => {
    if (endReached) {
      endReachedRef.current?.();
    }
  }, [endReached, items.length, loading]);

  // A replaced list starts at the top. Declared before the resync below, because on the commit that
  // both replace a list and move the offset the reset is the truth and the resync should read it.
  useEffect(() => {
    const element = scroller.current;
    if (element === null) {
      return;
    }
    element.scrollTop = 0;
    setScrollTop(0);
  }, [resetKey]);

  // **The scroller's position is refetched when the content changes**, because the browser clamps an
  // offset past the end silently: a list that shrank under this component leaves the element scrolled
  // to where it could still be while this copy of the offset is the old, larger number. That draws a
  // slice from the wrong place and can report the end of the list before the reader has seen its
  // start. Reading it back is one property access and React bails out when the number is unchanged.
  useEffect(() => {
    const element = scroller.current;
    if (element !== null) {
      setScrollTop(element.scrollTop);
    }
  }, [cells]);

  // One cell at a time, because the drawn window runs past the items while a page is reserved — a
  // slice cannot express "the last rows are placeholders".
  const drawn: ReactNode[] = [];
  for (let index = window_.first; index < window_.last; index += 1) {
    const item = items[index];
    if (item === undefined) {
      drawn.push(
        <div aria-hidden="true" key={`placeholder-${String(index)}`} role="listitem">
          {placeholder}
        </div>,
      );
      continue;
    }
    drawn.push(
      <div key={itemKey(item)} role="listitem">
        {renderItem(item)}
      </div>,
    );
  }

  return (
    <div
      aria-label={label}
      // `h-full` is load-bearing rather than cosmetic: `overflow-y-auto` only becomes a scroll
      // container when the box is height-constrained, and an unconstrained one grows to its content —
      // which is the 180 000 px spacer, so the "viewport" would be the whole grid and every tile would
      // be rendered. The caller sizes the parent (`h-80`, `flex-1 min-h-0`, or a protocol-sized pane)
      // and this fills it.
      //
      // **`overflow-x-hidden`, because this grid never scrolls sideways.** `overflow-y-auto` computes
      // `overflow-x` to `auto` as well, so a row one pixel wider than the measured viewport — which a
      // window resize between two measurements can produce — would give the grid a horizontal
      // scrollbar and let the whole page pan. The column count is derived to fit; anything left over
      // is a measurement race and must be clipped rather than scrolled.
      className={twMerge(
        clsx("h-full overflow-x-hidden overflow-y-auto overscroll-y-contain rounded-md"),
        className,
      )}
      ref={scroller}
      role="list"
      // A scroller that a keyboard cannot reach is a scroller a keyboard cannot scroll (PG-E7-19's
      // model puts the arrows inside the grid; until then the region itself takes focus).
      tabIndex={0}
    >
      {/* The spacer's height is the whole grid's, in one element: that is what keeps the scrollbar
          honest while the nodes for the tiles that are not on screen do not exist. */}
      <div className="relative" style={{ height: `${String(totalRows * rowHeight)}px` }}>
        <div
          className={clsx("grid", scrolling && "will-change-transform")}
          style={{
            gap: `${String(gap)}px`,
            gridTemplateColumns: `repeat(${String(columns)}, ${String(tileWidth)}px)`,
            transform: `translateY(${String(window_.offset)}px)`,
          }}
        >
          {drawn}
        </div>
      </div>
    </div>
  );
}
