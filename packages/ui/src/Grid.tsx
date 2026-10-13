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
  const window_ = gridWindow({
    columns,
    itemCount: items.length,
    overscanRows,
    rowHeight,
    scrollTop,
    viewportHeight: viewport.height,
  });
  const totalRows = items.length === 0 ? 0 : Math.ceil(items.length / columns);
  const slice = items.slice(window_.first, window_.last);

  return (
    <div
      aria-label={label}
      // `h-full` is load-bearing rather than cosmetic: `overflow-y-auto` only becomes a scroll
      // container when the box is height-constrained, and an unconstrained one grows to its content —
      // which is the 180 000 px spacer, so the "viewport" would be the whole grid and every tile would
      // be rendered. The caller sizes the parent (`h-80`, `flex-1 min-h-0`, or a protocol-sized pane)
      // and this fills it.
      className={twMerge(clsx("h-full overflow-y-auto overscroll-y-contain rounded-md"), className)}
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
          {slice.map((item) => (
            <div key={itemKey(item)} role="listitem">
              {renderItem(item)}
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
