/**
 * The profiles this build has.
 *
 * Names and colours only: accounts, per-profile content, and parental gating are Phase 8 (ADR 0019).
 * Until that backend exists this is a device-local list so adding, renaming, and choosing a profile
 * are real actions rather than disabled tiles. Nothing here is a security boundary.
 */
export interface Profile {
  /** Stable within this device, so a rename does not reselect somebody else. */
  readonly id: string;
  /** The name shown under the avatar. */
  readonly name: string;
  /** The avatar's hue, 0-359; derived from the name when it is created. */
  readonly hue: number;
}

/** Every profile on this device, and which one is watching. */
export interface ProfileBook {
  readonly profiles: readonly Profile[];
  readonly activeId: string;
}

const KEY = "peregrine.profiles";
const LEGACY_KEY = "peregrine.profile";

/** The profile a fresh install starts with. */
export const DEFAULT_PROFILE: Profile = { id: "you", name: "You", hue: 212 };

/** The book a fresh install starts with. */
export const DEFAULT_BOOK: ProfileBook = {
  profiles: [DEFAULT_PROFILE],
  activeId: DEFAULT_PROFILE.id,
};

/** A hue derived from the name, so the same name always draws the same avatar. */
export function hueFor(name: string): number {
  let hash = 0;
  for (const character of name) {
    hash = (hash * 31 + (character.codePointAt(0) ?? 0)) % 360;
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

/** A new profile for a name, with an id that is stable for the life of this device. */
export function newProfile(name: string): Profile {
  const id =
    typeof crypto.randomUUID === "function" ? crypto.randomUUID() : `profile-${String(Date.now())}`;
  return { id, name: name.trim(), hue: hueFor(name) };
}

function validProfile(value: unknown): value is Profile {
  if (typeof value !== "object" || value === null) return false;
  const { id, name, hue } = value as Record<string, unknown>;
  return (
    typeof id === "string" &&
    id.length > 0 &&
    typeof name === "string" &&
    name.trim().length > 0 &&
    typeof hue === "number" &&
    hue >= 0 &&
    hue < 360
  );
}

function validBook(value: unknown): value is ProfileBook {
  if (typeof value !== "object" || value === null) return false;
  const { profiles, activeId } = value as Record<string, unknown>;
  return (
    Array.isArray(profiles) &&
    profiles.length > 0 &&
    profiles.every(validProfile) &&
    typeof activeId === "string" &&
    profiles.some((profile) => (profile as Profile).id === activeId)
  );
}

/** The stored book, migrating the single-profile value an earlier build wrote. */
export function loadProfileBook(): ProfileBook {
  try {
    const raw = globalThis.localStorage?.getItem(KEY);
    if (raw !== null && raw !== undefined) {
      const parsed: unknown = JSON.parse(raw);
      if (validBook(parsed)) return parsed;
    }
    const legacy = globalThis.localStorage?.getItem(LEGACY_KEY);
    if (legacy !== null && legacy !== undefined) {
      const parsed: unknown = JSON.parse(legacy);
      if (
        typeof parsed === "object" &&
        parsed !== null &&
        typeof (parsed as Record<string, unknown>).name === "string"
      ) {
        const { name, hue } = parsed as { name: string; hue?: number };
        const profile: Profile = {
          id: DEFAULT_PROFILE.id,
          name,
          hue: typeof hue === "number" ? hue : hueFor(name),
        };
        return { profiles: [profile], activeId: profile.id };
      }
    }
    return DEFAULT_BOOK;
  } catch {
    return DEFAULT_BOOK;
  }
}

/** Stores the book, ignoring a storage that refuses writes. */
export function saveProfileBook(book: ProfileBook): void {
  try {
    globalThis.localStorage?.setItem(KEY, JSON.stringify(book));
  } catch {
    // The session still works; the next launch starts from the default.
  }
}

/** The active profile, falling back to the first when a stored id no longer exists. */
export function activeProfile(book: ProfileBook): Profile {
  return book.profiles.find((profile) => profile.id === book.activeId) ?? book.profiles[0]!;
}
