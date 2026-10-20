/** The colour and initials a profile without a picture falls back to. */

/** A hue derived from the name, so the same name always draws the same colour. */
export function hueFor(name: string): number {
  let hash = 0;
  for (const character of name) {
    hash = (hash * 31 + (character.codePointAt(0) ?? 0)) % 360;
  }
  return hash;
}

/** Up to two initials for the avatar. */
export function initialsFor(name: string): string {
  const words = name
    .trim()
    .split(/\s+/u)
    .filter((word) => word.length > 0);
  const first = words[0]?.[0] ?? "?";
  const last = words.length > 1 ? (words[words.length - 1]?.[0] ?? "") : "";
  return (first + last).toUpperCase();
}

/** A stored data URL, or null when the profile has no picture and draws initials. */
export function avatarSrc(avatarRef: string | null): string | null {
  return avatarRef !== null && avatarRef.startsWith("data:image/") ? avatarRef : null;
}
