import { createAvatar } from "@dicebear/core";
import {
  adventurer,
  bottts,
  funEmoji,
  initials,
  lorelei,
  notionists,
  shapes,
  thumbs,
} from "@dicebear/collection";

/**
 * The profiles this build has.
 *
 * Names, colours, and avatars only: accounts, per-profile content, and parental gating are Phase 8
 * (ADR 0019). Until that backend exists this is a device-local list so adding, renaming, choosing an
 * avatar, and choosing a profile are real actions. Nothing here is a security boundary.
 */
export type AvatarStyle =
  | "initials"
  | "thumbs"
  | "bottts"
  | "funEmoji"
  | "notionists"
  | "adventurer"
  | "lorelei"
  | "shapes";

/** Where a profile's picture comes from. */
export type Avatar =
  | { readonly kind: "dicebear"; readonly style: AvatarStyle; readonly seed: string }
  | { readonly kind: "upload"; readonly dataUrl: string };

export interface Profile {
  /** Stable within this device, so a rename does not reselect somebody else. */
  readonly id: string;
  /** The name shown under the avatar. */
  readonly name: string;
  /** A fallback hue, used when a generated avatar cannot be drawn. */
  readonly hue: number;
  readonly avatar: Avatar;
}

/** Every profile on this device, and which one is watching. */
export interface ProfileBook {
  readonly profiles: readonly Profile[];
  readonly activeId: string;
}

const KEY = "peregrine.profiles";
const LEGACY_KEY = "peregrine.profile";

/** The DiceBear styles offered, with the labels the editor draws. */
export const AVATAR_STYLES = {
  initials: initials,
  thumbs: thumbs,
  bottts: bottts,
  funEmoji: funEmoji,
  notionists: notionists,
  adventurer: adventurer,
  lorelei: lorelei,
  shapes: shapes,
} as const;

/** The style keys, in the order the editor draws them. */
export const AVATAR_STYLE_KEYS = Object.keys(AVATAR_STYLES) as AvatarStyle[];

export const AVATAR_STYLE_LABELS: Readonly<Record<AvatarStyle, string>> = {
  initials: "Initials",
  thumbs: "Thumbs",
  bottts: "Robots",
  funEmoji: "Faces",
  notionists: "Sketches",
  adventurer: "Adventurers",
  lorelei: "Portraits",
  shapes: "Shapes",
};

/** The avatar a fresh profile starts with. */
export const DEFAULT_AVATAR: Avatar = { kind: "dicebear", style: "initials", seed: "You" };

/** The profile a fresh install starts with. */
export const DEFAULT_PROFILE: Profile = {
  id: "you",
  name: "You",
  hue: 212,
  avatar: DEFAULT_AVATAR,
};

/** The book a fresh install starts with. */
export const DEFAULT_BOOK: ProfileBook = {
  profiles: [DEFAULT_PROFILE],
  activeId: DEFAULT_PROFILE.id,
};

/** A hue derived from the name, so the same name always draws the same fallback colour. */
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

/** The data URI a generated avatar is drawn from. */
export function avatarUri(avatar: Avatar, size = 128): string {
  if (avatar.kind === "upload") return avatar.dataUrl;
  // Each style module has its own option type, so the map's values form a union the generic
  // cannot infer through. The runtime call is the documented one; only the type needs the cast.
  return createAvatar(AVATAR_STYLES[avatar.style] as Parameters<typeof createAvatar>[0], {
    seed: avatar.seed,
    size,
    radius: 8,
  }).toDataUri();
}

/** A new profile for a name, with an id that is stable for the life of this device. */
export function newProfile(name: string, avatar?: Avatar): Profile {
  const trimmed = name.trim();
  const id =
    typeof crypto.randomUUID === "function" ? crypto.randomUUID() : `profile-${String(Date.now())}`;
  return {
    id,
    name: trimmed,
    hue: hueFor(trimmed),
    avatar: avatar ?? { kind: "dicebear", style: "initials", seed: trimmed },
  };
}

function validAvatar(value: unknown): value is Avatar {
  if (typeof value !== "object" || value === null) return false;
  const { kind, style, seed, dataUrl } = value as Record<string, unknown>;
  if (kind === "upload") return typeof dataUrl === "string" && dataUrl.startsWith("data:image/");
  return (
    kind === "dicebear" &&
    typeof style === "string" &&
    AVATAR_STYLE_KEYS.includes(style as AvatarStyle) &&
    typeof seed === "string"
  );
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

/** The profile as stored, with the avatar an earlier build did not have. */
function withAvatar(value: Profile): Profile {
  return validAvatar(value.avatar)
    ? value
    : { ...value, avatar: { kind: "dicebear", style: "initials", seed: value.name } };
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
      if (validBook(parsed)) {
        return {
          profiles: parsed.profiles.map((profile) => withAvatar(profile)),
          activeId: parsed.activeId,
        };
      }
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
          avatar: { kind: "dicebear", style: "initials", seed: name },
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
