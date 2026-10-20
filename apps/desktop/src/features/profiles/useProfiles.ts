import { useCallback, useEffect, useState } from "react";

import { createProfile, listProfiles, selectProfile, updateProfile } from "@peregrine/ipc-client";

import {
  activeProfile,
  avatarUri,
  hueFor,
  loadProfileBook,
  saveProfileBook,
  type Avatar,
  type Profile,
  type ProfileBook,
} from "./profile.js";

/** The profiles this device holds, and the actions the selector can take. */
export interface ProfilesState {
  readonly book: ProfileBook;
  readonly active: Profile;
  readonly select: (id: string) => void;
  readonly add: (name: string, avatar: Avatar) => void;
  readonly rename: (id: string, name: string, avatar: Avatar) => void;
  readonly refresh: () => void;
}

/** One server row as the selector's local shape. */
function asProfile(row: {
  readonly id: number;
  readonly name: string;
  readonly avatarRef: string | null;
}): Profile {
  const avatar: Avatar =
    row.avatarRef !== null && row.avatarRef.startsWith("data:")
      ? { kind: "upload", dataUrl: row.avatarRef }
      : { kind: "dicebear", style: "initials", seed: row.name };
  return {
    id: String(row.id),
    name: row.name,
    hue: hueFor(row.name),
    avatar,
  };
}

function asBook(
  rows: readonly {
    readonly id: number;
    readonly name: string;
    readonly avatarRef: string | null;
    readonly active: boolean;
  }[],
): ProfileBook | null {
  if (rows.length === 0) return null;
  const profiles = rows.map(asProfile);
  const active = rows.find((row) => row.active) ?? rows[0];
  if (active === undefined) return null;
  return { profiles, activeId: String(active.id) };
}

/**
 * The household profiles, read from the core.
 *
 * The core owns the list, the active choice, and the watch state scoped by it; this hook only
 * maps rows to the selector's drawing. A profile added in a browser appears here on the next
 * read, and selecting one here changes what every scoped read returns.
 */
export function useProfiles(): ProfilesState {
  const [book, setBook] = useState<ProfileBook>(loadProfileBook);

  const load = useCallback(async (): Promise<void> => {
    try {
      const rows = await listProfiles();
      const next = asBook(rows);
      if (next !== null) {
        setBook(next);
        saveProfileBook(next);
      }
    } catch {
      // The selector keeps the last known list; a dead runtime is already reported by the shell.
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  // A profile chosen on another client is the installation's choice too. Reload rather than
  // redraw the rails under a scope none of them were read with.
  useEffect(() => {
    const timer = setInterval(() => {
      void listProfiles()
        .then((rows) => {
          const active = rows.find((row) => row.active);
          if (active !== undefined && String(active.id) !== book.activeId) {
            globalThis.location?.reload();
          }
        })
        .catch(() => {
          // A read that failed is not a switch; the next tick asks again.
        });
    }, 10_000);
    return () => {
      clearInterval(timer);
    };
  }, [book.activeId]);

  const select = useCallback(
    (id: string) => {
      const numeric = Number(id);
      if (!Number.isSafeInteger(numeric) || numeric <= 0) return;
      setBook((current) => {
        if (!current.profiles.some((profile) => profile.id === id)) return current;
        const next = { ...current, activeId: id };
        saveProfileBook(next);
        return next;
      });
      void selectProfile({ id: numeric })
        .then(() => {
          // Every screen's rails and marks are scoped, so a switch starts from a fresh read.
          globalThis.location?.reload();
        })
        .catch(() => {
          void load();
        });
    },
    [load],
  );

  const add = useCallback(
    (name: string, avatar: Avatar) => {
      const trimmed = name.trim();
      if (trimmed.length === 0) return;
      void createProfile(trimmed, "adult", avatarUri(avatar, 128))
        .then((row) => {
          setBook((current) => {
            const next = {
              profiles: [...current.profiles, asProfile(row)],
              activeId: String(row.id),
            };
            saveProfileBook(next);
            return next;
          });
          globalThis.location?.reload();
        })
        .catch(() => {
          void load();
        });
    },
    [load],
  );

  const rename = useCallback(
    (id: string, name: string, avatar: Avatar) => {
      const trimmed = name.trim();
      const numeric = Number(id);
      if (trimmed.length === 0 || !Number.isSafeInteger(numeric) || numeric <= 0) return;
      void updateProfile({ id: numeric }, trimmed, avatarUri(avatar, 128))
        .then((row) => {
          setBook((current) => {
            const next = {
              ...current,
              profiles: current.profiles.map((profile) =>
                profile.id === id ? asProfile(row) : profile,
              ),
            };
            saveProfileBook(next);
            return next;
          });
        })
        .catch(() => {
          void load();
        });
    },
    [load],
  );

  return {
    book,
    active: activeProfile(book),
    select,
    add,
    rename,
    refresh: () => {
      void load();
    },
  };
}
