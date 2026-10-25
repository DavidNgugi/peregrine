/** The colour and initials a profile without a picture falls back to. */
import { createAvatar } from "@dicebear/core";
import { initials } from "@dicebear/collection";

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
export function avatarSrc(avatarRef: string | null, name?: string): string | null {
  if (avatarRef !== null && avatarRef.startsWith("data:image/")) return avatarRef;
  if (name === undefined) return null;
  return createAvatar(initials, { seed: name, size: 128, radius: 8 }).toDataUri();
}

/** One profile from the device-local book the earlier web picker wrote. */
export interface LegacyProfile {
  readonly name: string;
  readonly active: boolean;
}

/** The old browser-local book, when one is still in storage. */
export function legacyProfiles(): LegacyProfile[] {
  try {
    const raw = globalThis.localStorage?.getItem("peregrine.profiles");
    if (raw === null || raw === undefined) return [];
    const value = JSON.parse(raw) as {
      profiles?: unknown;
      activeId?: unknown;
    };
    if (!Array.isArray(value.profiles)) return [];
    return value.profiles.flatMap((entry) => {
      if (typeof entry !== "object" || entry === null) return [];
      const { id, name } = entry as { id?: unknown; name?: unknown };
      if (typeof id !== "string" || typeof name !== "string" || name.trim() === "") return [];
      return [{ name: name.trim(), active: id === value.activeId }];
    });
  } catch {
    return [];
  }
}

/** Drops the old book once its profiles have been written to the server. */
export function clearLegacyProfiles(): void {
  globalThis.localStorage?.removeItem("peregrine.profiles");
  globalThis.localStorage?.removeItem("peregrine.profile");
}
