/**
 * The profiles this browser holds.
 *
 * These are device-local labels, exactly like the desktop's: they do not scope the catalogue,
 * watch progress, or the watchlist. Accounts and per-profile content are ADR 0019's, still ahead.
 * The storage key matches the desktop so the shape stays familiar; the browsers do not share an
 * origin, so each keeps its own list.
 */

/** One device-local profile. */
export interface WebProfile {
  readonly id: string;
  readonly name: string;
  readonly hue: number;
}

/** Every profile on this browser, and which one is chosen. */
export interface ProfileBook {
  readonly profiles: readonly WebProfile[];
  readonly activeId: string;
}

const KEY = "peregrine.profiles";

/** The profile a browser with no stored list starts on. */
export const DEFAULT_PROFILE: WebProfile = { id: "you", name: "You", hue: 212 };

/** The book a fresh browser starts with. */
export const DEFAULT_BOOK: ProfileBook = {
  profiles: [DEFAULT_PROFILE],
  activeId: DEFAULT_PROFILE.id,
};

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

function validProfile(value: unknown): value is WebProfile {
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

/** Reads a stored book, replacing anything malformed with the default. */
export function parseProfileBook(raw: string | null): ProfileBook {
  if (raw === null) return DEFAULT_BOOK;
  try {
    const value = JSON.parse(raw) as unknown;
    if (typeof value !== "object" || value === null) return DEFAULT_BOOK;
    const { profiles, activeId } = value as Record<string, unknown>;
    if (!Array.isArray(profiles) || profiles.length === 0) return DEFAULT_BOOK;
    const valid = profiles.filter(validProfile);
    if (valid.length === 0) return DEFAULT_BOOK;
    return {
      profiles: valid,
      activeId:
        typeof activeId === "string" && valid.some((profile) => profile.id === activeId)
          ? activeId
          : (valid[0]?.id ?? DEFAULT_PROFILE.id),
    };
  } catch {
    return DEFAULT_BOOK;
  }
}

/** Reads the browser's book. */
export function loadProfileBook(): ProfileBook {
  if (typeof localStorage === "undefined") return DEFAULT_BOOK;
  return parseProfileBook(localStorage.getItem(KEY));
}

/** Stores the book. */
export function saveProfileBook(book: ProfileBook): void {
  if (typeof localStorage === "undefined") return;
  localStorage.setItem(KEY, JSON.stringify(book));
}

/** A new profile for a name, or the same book when the name is blank. */
export function withProfile(book: ProfileBook, name: string): ProfileBook {
  const trimmed = name.trim();
  if (trimmed.length === 0) return book;
  const profile: WebProfile = {
    id:
      typeof crypto !== "undefined" && typeof crypto.randomUUID === "function"
        ? crypto.randomUUID()
        : `profile-${String(Date.now())}`,
    name: trimmed,
    hue: hueFor(trimmed),
  };
  return { profiles: [...book.profiles, profile], activeId: profile.id };
}

/** Selects an existing profile, or the same book when the id is unknown. */
export function selecting(book: ProfileBook, id: string): ProfileBook {
  if (!book.profiles.some((profile) => profile.id === id)) return book;
  return { ...book, activeId: id };
}

/** The chosen profile, or the first one when the id no longer names a row. */
export function activeProfile(book: ProfileBook): WebProfile {
  return book.profiles.find((profile) => profile.id === book.activeId) ?? DEFAULT_PROFILE;
}
