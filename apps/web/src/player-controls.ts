/**
 * The remote's grammar inside the player, kept pure so a held key and a scrub can be tested
 * without a television.
 *
 * A remote has five keys that matter and no pointer, so the player gives them one meaning each
 * while the controls are hidden — OK pauses, left and right scrub, up and down reveal — and hands
 * them back to spatial focus once the controls are showing. A held arrow on Tizen arrives as a
 * stream of keydowns whose `repeat` flag the engine does not always set, so a run is measured by
 * the gap between presses rather than trusted from the event.
 */

/** A remote key the player gives a meaning to. */
export type PlayerKey = "ok" | "left" | "right" | "up" | "down";

/** The player key a keydown carries, or null for every other key. */
export function playerKey(key: string, keyCode: number): PlayerKey | null {
  switch (key) {
    case "Enter":
      return "ok";
    case "ArrowLeft":
      return "left";
    case "ArrowRight":
      return "right";
    case "ArrowUp":
      return "up";
    case "ArrowDown":
      return "down";
  }
  // Some Tizen builds report the D-pad by code with an empty or "Unidentified" name.
  switch (keyCode) {
    case 13:
      return "ok";
    case 37:
      return "left";
    case 39:
      return "right";
    case 38:
      return "up";
    case 40:
      return "down";
    default:
      return null;
  }
}

/** One held scrub: its direction, how many presses it has had, and when the last one landed. */
export interface ScrubRun {
  readonly direction: -1 | 1;
  readonly count: number;
  readonly atMs: number;
}

/**
 * The gap that still counts as holding the key. Tizen repeats at roughly 50–100 ms; a deliberate
 * second press is slower than this, so it starts a new run at the smallest step.
 */
export const SCRUB_HOLD_GAP_MS = 350;

/** The run after one more press in `direction` at `nowMs`. */
export function nextScrubRun(previous: ScrubRun | null, direction: -1 | 1, nowMs: number): ScrubRun {
  if (
    previous !== null &&
    previous.direction === direction &&
    nowMs - previous.atMs >= 0 &&
    nowMs - previous.atMs <= SCRUB_HOLD_GAP_MS
  ) {
    return { direction, count: previous.count + 1, atMs: nowMs };
  }
  return { direction, count: 0, atMs: nowMs };
}

/**
 * How far one press moves the scrub, growing the longer the key is held.
 *
 * `base` is the first step: ten seconds for an arrow, thirty for the remote's fast-forward and
 * rewind keys. A two-hour film is crossed in a few seconds of holding without making a single tap
 * imprecise.
 */
export function scrubStepMs(count: number, base = 10_000): number {
  if (count < 6) return base;
  if (count < 16) return base * 3;
  if (count < 30) return base * 6;
  return base * 12;
}

/** Where a scrub lands, clamped to the media; an unknown length clamps only at zero. */
export function scrubTarget(fromMs: number, deltaMs: number, totalMs: number | null): number {
  const target = Math.max(0, fromMs + deltaMs);
  return totalMs === null ? target : Math.min(target, Math.max(0, totalMs - 1000));
}

/**
 * The media-element time that reaches `targetMs` without a new session, or null when only a
 * restart can reach it.
 *
 * A direct file is the whole title, so every position is a `currentTime` away. A transcode's clock
 * starts at its offset and ends where the encoder has got to: a target inside that window is
 * already on disk and seeks instantly, while one before the offset or past the encoder's edge needs
 * a session started there. The one-second margin keeps a seek off the segment still being written.
 */
export function inSessionSeconds(
  kind: "direct" | "hls",
  startPositionMs: number,
  targetMs: number,
  seekableEndSeconds: number | null,
): number | null {
  if (!Number.isFinite(targetMs) || targetMs < 0) return null;
  if (kind === "direct") return targetMs / 1000;
  const relativeMs = targetMs - Math.max(0, startPositionMs);
  if (relativeMs < 0) return null;
  if (seekableEndSeconds === null || !Number.isFinite(seekableEndSeconds)) return null;
  const seconds = relativeMs / 1000;
  return seconds <= seekableEndSeconds - 1 ? seconds : null;
}

/** How long the controls stay up after the last key while the video plays. */
export const CONTROLS_IDLE_MS = 4000;

/** How long a scrub waits for another press before it seeks. */
export const SCRUB_COMMIT_MS = 700;

/** The timeline's fill, as a whole-percent width the stylesheet can use. */
export function progressPercent(positionMs: number, totalMs: number | null): number {
  if (totalMs === null || totalMs <= 0 || !Number.isFinite(positionMs)) return 0;
  return Math.min(100, Math.max(0, (positionMs / totalMs) * 100));
}
