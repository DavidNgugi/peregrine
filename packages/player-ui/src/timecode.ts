/** Timecode and progress formatting for the player surface. Presentation only. */

const MILLISECONDS_PER_SECOND = 1000;
const SECONDS_PER_MINUTE = 60;
const SECONDS_PER_HOUR = 3600;

/**
 * `m:ss`, or `h:mm:ss` once the position passes an hour.
 *
 * Non-finite and negative positions render as `0:00`: a stalled or reset session
 * must not print `NaN` into the transport.
 */
export function formatTimecode(positionMs: number): string {
  if (!Number.isFinite(positionMs)) {
    return "0:00";
  }
  const totalSeconds = Math.max(0, Math.floor(positionMs / MILLISECONDS_PER_SECOND));
  const hours = Math.floor(totalSeconds / SECONDS_PER_HOUR);
  const minutes = Math.floor((totalSeconds % SECONDS_PER_HOUR) / SECONDS_PER_MINUTE);
  const seconds = totalSeconds % SECONDS_PER_MINUTE;
  const paddedMinutes = String(minutes).padStart(2, "0");
  const paddedSeconds = String(seconds).padStart(2, "0");
  return hours > 0
    ? `${String(hours)}:${paddedMinutes}:${paddedSeconds}`
    : `${String(minutes)}:${paddedSeconds}`;
}

/**
 * How far through the media the position is, clamped to `0..1`.
 *
 * A zero, negative, or unknown duration reports `0` rather than dividing by zero,
 * which is the state of a session whose probe has not finished yet.
 */
export function progressFraction(positionMs: number, durationMs: number): number {
  if (!Number.isFinite(positionMs) || !Number.isFinite(durationMs) || durationMs <= 0) {
    return 0;
  }
  return Math.min(1, Math.max(0, positionMs / durationMs));
}
