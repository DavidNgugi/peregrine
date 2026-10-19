/**
 * Where a D-pad press moves focus, as a pure function.
 *
 * The DOM half -- reading boxes and calling focus() -- is a hook in App.tsx. This half is here because
 * it is the part that can be wrong in a way a television shows and a unit test can hold still: a
 * press must reach the nearest control in that direction and never jump a row, and the edge of a grid
 * must keep focus rather than wrap to something off-screen.
 */

export type Direction = "up" | "down" | "left" | "right";

/** A control's box in viewport coordinates. */
export interface Rect {
  readonly x: number;
  readonly y: number;
  readonly width: number;
  readonly height: number;
}

/**
 * The next control in `direction`, or `current` when there is none.
 *
 * Ties go to the lower index, so a symmetric grid always resolves the same way and two presses do not
 * alternate between equally plausible tiles.
 */
export function nextIndex(rects: readonly Rect[], current: number, direction: Direction): number {
  const from = rects[current];
  if (from === undefined) {
    return current;
  }
  const cx = from.x + from.width / 2;
  const cy = from.y + from.height / 2;
  let best = -1;
  let bestScore = Number.POSITIVE_INFINITY;
  for (let index = 0; index < rects.length; index += 1) {
    if (index === current) continue;
    const rect = rects[index];
    if (rect === undefined) continue;
    const dx = rect.x + rect.width / 2 - cx;
    const dy = rect.y + rect.height / 2 - cy;
    const score = scoreFor(direction, dx, dy);
    if (score !== null && score < bestScore) {
      best = index;
      bestScore = score;
    }
  }
  return best === -1 ? current : best;
}

/**
 * How far a candidate is in `direction`, weighted so a tile in the same row beats a nearer one
 * diagonally beside it. `null` when the candidate is not in that half-plane at all.
 */
function scoreFor(direction: Direction, dx: number, dy: number): number | null {
  const along =
    direction === "right" ? dx : direction === "left" ? -dx : direction === "down" ? dy : -dy;
  const across = Math.abs(direction === "left" || direction === "right" ? dy : dx);
  if (along <= 1) {
    return null;
  }
  return along + across * 2;
}
