import { useCallback, useState } from "react";

import {
  activeProfile,
  loadProfileBook,
  newProfile,
  saveProfileBook,
  type Profile,
  type ProfileBook,
} from "./profile.js";

/** The profiles this device holds, and the actions the selector can take. */
export interface ProfilesState {
  readonly book: ProfileBook;
  readonly active: Profile;
  readonly select: (id: string) => void;
  readonly add: (name: string) => void;
  readonly rename: (id: string, name: string) => void;
}

export function useProfiles(): ProfilesState {
  const [book, setBook] = useState<ProfileBook>(loadProfileBook);
  const select = useCallback((id: string) => {
    setBook((current) => {
      if (!current.profiles.some((profile) => profile.id === id)) return current;
      const next = { ...current, activeId: id };
      saveProfileBook(next);
      return next;
    });
  }, []);
  const add = useCallback((name: string) => {
    const trimmed = name.trim();
    if (trimmed.length === 0) return;
    setBook((current) => {
      const profile = newProfile(trimmed);
      const next = { profiles: [...current.profiles, profile], activeId: profile.id };
      saveProfileBook(next);
      return next;
    });
  }, []);
  const rename = useCallback((id: string, name: string) => {
    const trimmed = name.trim();
    if (trimmed.length === 0) return;
    setBook((current) => {
      const next = {
        ...current,
        profiles: current.profiles.map((profile) =>
          profile.id === id ? { ...profile, name: trimmed } : profile,
        ),
      };
      saveProfileBook(next);
      return next;
    });
  }, []);
  return { book, active: activeProfile(book), select, add, rename };
}
