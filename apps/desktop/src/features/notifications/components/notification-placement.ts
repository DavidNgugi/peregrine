/** Where the notification panel can sit without crossing a viewport edge. */

/** The part of the bell's rect the placement needs. */
export interface AnchorRect {
  readonly right: number;
  readonly bottom: number;
}

/** The panel's viewport box. */
export interface PanelPlacement {
  readonly top: number;
  readonly left: number;
  readonly width: number;
  readonly maxHeight: number;
}

const MARGIN = 8;
const MAX_WIDTH = 320;
const MIN_WIDTH = 180;

/**
 * Clamps the panel to the viewport.
 *
 * The preferred position is right-aligned to the bell; when that would put the left edge
 * off-screen the panel moves right until it fits, and on a very narrow window it shrinks rather
 * than overflowing. The top is clamped so a short window still leaves rows visible.
 */
export function panelPlacement(
  anchor: AnchorRect | null,
  viewportWidth: number,
  viewportHeight: number,
): PanelPlacement {
  const width = Math.max(MIN_WIDTH, Math.min(MAX_WIDTH, viewportWidth - MARGIN * 2));
  const right = anchor?.right ?? viewportWidth - MARGIN;
  // Prefer aligning the panel's right edge to the bell. When that would push its left edge
  // off-screen, move the panel right until it fits, so a narrow window keeps it near the bell
  // rather than dropping it at the far-left edge.
  let left = right - width;
  if (left < MARGIN) left = Math.min(right, viewportWidth - width - MARGIN);
  left = Math.max(MARGIN, left);
  const top = Math.max(MARGIN, Math.min((anchor?.bottom ?? 64) + MARGIN, viewportHeight - 168));
  return {
    top,
    left,
    width,
    maxHeight: Math.max(160, viewportHeight - top - MARGIN),
  };
}
