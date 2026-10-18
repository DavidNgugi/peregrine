/**
 * Choosing which sidecar subtitle the player shows.
 *
 * **Pure, so the one rule a screen cannot read off the session has a test without a DOM.** The core
 * decides *which* track is the default (`subtitle_track.is_default`); this module only reads that
 * answer, and names a track the way a menu entry reads. A "remember this language" rule is
 * `PG-E17-12`/`PG-E17-13` and deliberately not here.
 */

import type { SubtitleTrackDto } from "@peregrine/ipc-client";

/**
 * The track the player shows when a session opens.
 *
 * The core's default flag, or `null` for "off": a file whose sidecars the scan found but never chose
 * between opens with no subtitle, which is the same option a reader gets from the menu's first entry.
 */
export function initialSubtitleTrack(tracks: readonly SubtitleTrackDto[]): number | null {
  return tracks.find((track) => track.default)?.id ?? null;
}

/**
 * The text one menu entry shows.
 *
 * The core's label wins over the language tag, and the flags and the container are appended because a
 * reader choosing between two "English" rows needs to see which one is forced, and because the
 * webview's track parser reads WebVTT rather than every container the library can hold.
 */
export function subtitleTrackLabel(track: SubtitleTrackDto): string {
  const name = track.label?.trim() || track.language;
  const markers = [
    track.forced ? "Forced" : null,
    track.sdh ? "SDH" : null,
    track.format.toUpperCase(),
  ].filter((marker): marker is string => marker !== null);
  return `${name} · ${markers.join(" · ")}`;
}
