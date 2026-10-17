/**
 * The one profile this build has.
 *
 * Accounts, avatars, and per-profile content are Phase 8 (ADR 0019). Until that backend exists, the
 * selector has a single local identity so the entry point and the switching flow are in place and the
 * account work can replace this module rather than the UI around it. The value is device-local and
 * holds nothing but a display name and a colour.
 */
export interface Profile {
  /** The name shown under the avatar. */
  readonly name: string;
  /** The avatar's hue, 0-359; derived from the name when one is chosen. */
  readonly hue: number;
}

const KEY = "peregrine.profile";

/** The profile a fresh install starts with. */
export const DEFAULT_PROFILE: Profile = { name: "You", hue: 212 };

/** A hue derived from the name, so the same name always draws the same avatar. */
export function hueFor(name: string): number {
  let hash = 0;
  for (const character of name) {
    hash = (hash * 31 + character.codePointAt(0)!) % 360;
  }
  return hash;
}

/** Up to two initials, which is what an avatar has room for. */
export function initialsFor(name: string): string {
  const words = name.trim().split(/\s+/).filter(Boolean);
  const first = words[0]?.[0] ?? "?";
  const second = words.length > 1 ? (words[words.length - 1]?.[0] ?? "") : "";
  return (first + second).toUpperCase();
}

function valid(value: unknown): value is Profile {
  if (typeof value !== "object" || value === null) return false;
  const { name, hue } = value as Record<string, unknown>;
  return (
    typeof name === "string" &&
    name.trim().length > 0 &&
    typeof hue === "number" &&
    hue >= 0 &&
    hue < 360
  );
}

/** The stored profile, or the default when nothing valid is stored. */
export function loadProfile(): Profile {
  try {
    const raw = globalThis.localStorage?.getItem(KEY);
    if (raw === undefined || raw === null) return DEFAULT_PROFILE;
    const parsed: unknown = JSON.parse(raw);
    return valid(parsed) ? parsed : DEFAULT_PROFILE;
  } catch {
    return DEFAULT_PROFILE;
  }
}

/** Stores the profile, ignoring a storage that refuses writes (private mode, a locked device). */
export function saveProfile(profile: Profile): void {
  try {
    globalThis.localStorage?.setItem(KEY, JSON.stringify(profile));
  } catch {
    // The selector still works for this session; the next launch starts from the default.
  }
}
