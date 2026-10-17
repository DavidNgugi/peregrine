import { useCallback, useState } from "react";

import { loadProfile, saveProfile, type Profile } from "./profile.js";

/** The profile this session shows, and a setter that persists it. */
export interface ProfileState {
  readonly profile: Profile;
  readonly setProfile: (next: Profile) => void;
}

export function useProfile(): ProfileState {
  const [profile, set] = useState<Profile>(loadProfile);
  const setProfile = useCallback((next: Profile) => {
    set(next);
    saveProfile(next);
  }, []);
  return { profile, setProfile };
}
