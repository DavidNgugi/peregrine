/**
 * The ticket-to-contract adapter for the player overlay.
 *
 * The web server speaks in flat marker rows and a next-episode row; `@peregrine/player-ui` speaks
 * in the shared `PlayerMarker`, `NextUp`, and post-play state machine. Keeping the translation
 * here, as pure functions, is what lets the marker lookup, the skip choice, and the countdown be
 * tested without a video element (AGENTS R13).
 */

import {
  activeMarker,
  type NextUp,
  type PlayerMarker,
  type PostPlayState,
} from "@peregrine/player-ui";

import type { Marker, NextEpisode } from "./api.js";

/** The label a skip control carries for one marker kind; credits are never skipped. */
const SKIP_LABELS = {
  intro: "Skip Intro",
  recap: "Skip Recap",
  preview: "Skip Preview",
} as const;

/** What a skip control should do at the current position. */
export interface SkipTarget {
  readonly label: string;
  readonly targetMs: number;
}

/**
 * Give each ticket marker the numeric identity the shared lookup requires.
 *
 * The ticket has no marker id because a marker is a fact about a title, not a row the player may
 * edit. Position in the resolved, start-ordered list is therefore the identity used here; it is
 * stable for the lifetime of one marker refresh, and the lookup never persists it.
 */
export function toPlayerMarkers(markers: readonly Marker[]): PlayerMarker[] {
  return markers.map((marker, index) => ({
    id: index + 1,
    kind: marker.kind,
    startMs: marker.startMs,
    endMs: marker.endMs,
    source: marker.source,
    confidence: marker.confidence,
  }));
}

/** The next-up offer the shared coordinator expects, or null at the end of a series or a film. */
export function toNextUp(next: NextEpisode | null, titleId: number): NextUp | null {
  if (next === null) return null;
  return {
    titleId,
    seasonNumber: next.seasonNumber,
    episodeNumber: next.episodeNumber,
    episodeId: next.episodeId,
    name: next.name,
  };
}

/** The absolute media position, whatever the delivery clock starts at. */
export function absolutePositionMs(
  kind: "direct" | "hls",
  startPositionMs: number,
  mediaSeconds: number,
): number {
  const mediaMs = Number.isFinite(mediaSeconds) ? Math.max(0, mediaSeconds * 1000) : 0;
  if (kind === "hls") return Math.max(0, startPositionMs) + mediaMs;
  // A direct element plays the file itself, so its clock already is the absolute position.
  return mediaMs;
}

/**
 * The skip offer at `positionMs`, or null outside every skippable marker.
 *
 * Credits are deliberately absent: the post-play coordinator owns the end of an episode, and a
 * "Skip credits" button would race the countdown. An open-ended marker cannot be skipped because
 * there is no end to seek to.
 */
export function skipTarget(
  markers: readonly PlayerMarker[],
  positionMs: number,
): SkipTarget | null {
  const active = activeMarker(markers, positionMs);
  if (active === null || active.kind === "credits" || active.endMs === null) return null;
  return { label: SKIP_LABELS[active.kind], targetMs: active.endMs };
}

/** The one-line name of the episode the countdown offers. */
export function nextUpLabel(nextUp: NextUp): string {
  const prefix = "S" + String(nextUp.seasonNumber) + " E" + String(nextUp.episodeNumber);
  const name = nextUp.name?.trim() ?? "";
  return name === "" ? prefix : prefix + " · " + name;
}

/**
 * The hidden post-play state with the next-up offer already known.
 *
 * The ticket carries the next episode before the boundary is reached, so the coordinator can open
 * from a credits marker or from `ended` without a second network read. `advance` still owns every
 * transition; this only seeds the value it reuses.
 */
export function hiddenPostPlay(nextUp: NextUp | null): PostPlayState {
  return { phase: "hidden", remainingMs: 0, nextUp, prefetch: "idle" };
}
