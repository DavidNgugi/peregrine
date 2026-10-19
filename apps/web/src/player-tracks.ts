/**
 * The two decisions the player makes from a ticket, kept pure so they have tests without a DOM:
 * which subtitle the menu starts on, and what the next-episode request is.
 *
 * The core has already chosen the default track and the audio language; this module only reads
 * those answers. "Always this language" is a preference the desktop owns, not a rule here.
 */

import type { NextEpisode, SubtitleTrack } from "./api.js";

/** The `textTracks` position the menu starts on, or null for Off. */
export function initialSubtitleIndex(tracks: readonly SubtitleTrack[]): number | null {
  const index = tracks.findIndex((track) => track.default);
  return index === -1 ? null : index;
}

/** One menu entry: the label the library gave, or the language, plus the flags that distinguish it. */
export function subtitleLabel(track: SubtitleTrack): string {
  const name = track.label?.trim() || track.language || "Track " + String(track.id);
  const markers = [track.forced ? "forced" : null, track.sdh ? "SDH" : null].filter(
    (marker): marker is string => marker !== null,
  );
  return markers.length === 0 ? name : name + " (" + markers.join(", ") + ")";
}

/** The request that starts the follow-up episode. */
export interface NextEpisodeRequest {
  readonly positionMs: 0;
  readonly seasonNumber: number;
  readonly episodeNumber: number;
}

/**
 * What to ask for once this ticket ends, or null when there is nothing to ask for: a film, or the
 * last playable episode. A ticket that carries no next episode is not an error, it is an ending.
 */
export function nextEpisodeRequest(next: NextEpisode | null): NextEpisodeRequest | null {
  if (next === null) return null;
  return {
    positionMs: 0,
    seasonNumber: next.seasonNumber,
    episodeNumber: next.episodeNumber,
  };
}

/**
 * The audio menu option for the language the host reported, falling back to the first rendition.
 *
 * Matching ignores case because `audioLanguage` and a manifest's `lang` tag come from different
 * sources (the account preference and the container) and are not guaranteed to agree on casing.
 */
export function audioOptionIndex(
  options: readonly { readonly language: string | null }[],
  preferred: string | null,
): number | null {
  if (options.length === 0) return null;
  if (preferred !== null) {
    const wanted = preferred.toLowerCase();
    const found = options.findIndex(
      (option) => option.language !== null && option.language.toLowerCase() === wanted,
    );
    if (found !== -1) return found;
  }
  return 0;
}
