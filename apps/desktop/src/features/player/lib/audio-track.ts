/**
 * Choosing the audio track a file should start on.
 *
 * The webview's `audioTracks` list is non-standard but WebKit exposes it; a Matroska file never
 * reaches this path, because mpv owns it. The rule is deliberately narrow: a preferred language
 * selects the first track whose tag matches it, and no match leaves the engine's own default alone.
 * A manual choice after playback starts is not undone — the caller applies this once per session.
 */

/** The subset of a browser audio track this decision needs. */
export interface AudioTrackLike {
  readonly language: string;
  enabled: boolean;
}

/** The subset of WebKit's `AudioTrackList` this decision needs. */
export interface AudioTrackListLike {
  readonly length: number;
  readonly [index: number]: AudioTrackLike;
}

/** The two-letter form of each language the Settings picker offers, and its ISO 639-2 code. */
const ISO_639_2: Readonly<Record<string, string>> = {
  de: "deu",
  en: "eng",
  es: "spa",
  fr: "fra",
  it: "ita",
  pt: "por",
};

/** Whether a track's language tag matches a preference.
 *
 * A preference is a BCP-47-ish tag the account chose; a container often carries the three-letter
 * ISO 639-2 form (`eng`) while the picker offers the two-letter form (`en`). Both directions are
 * accepted, and a subtag such as `en-GB` matches `en`. An untagged (`und`) track never matches.
 */
export function languageMatches(preferred: string, track: string): boolean {
  const wanted = preferred.trim().toLowerCase();
  const held = track.trim().toLowerCase();
  if (wanted === "" || held === "" || held === "und" || wanted === "und") {
    return false;
  }
  if (held === wanted) {
    return true;
  }
  if (held.startsWith(wanted + "-") || wanted.startsWith(held + "-")) {
    return true;
  }
  // A container commonly carries ISO 639-2 (\`eng\`) where the picker offers ISO 639-1 (\`en\`).
  // The table is the picker's own six languages; a language this build cannot offer in Settings
  // is not guessed at from a prefix.
  if (ISO_639_2[wanted] === held) {
    return true;
  }
  return Object.entries(ISO_639_2).some(([two, three]) => three === wanted && two === held);
}

/** The first index whose language matches, or `null` when none does. */
export function preferredTrackIndex(
  tracks: Pick<AudioTrackListLike, "length"> & Partial<AudioTrackListLike>,
  preferred: string,
): number | null {
  for (let index = 0; index < tracks.length; index += 1) {
    const track = tracks[index];
    if (track !== undefined && languageMatches(preferred, track.language)) {
      return index;
    }
  }
  return null;
}
