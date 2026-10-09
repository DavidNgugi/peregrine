import { DEFAULT_THEME, type ThemeName } from "@peregrine/theme";
import { createStore } from "zustand/vanilla";

/** `system` follows the OS; the other two are explicit user choices. */
export type ThemePreference = "system" | ThemeName;

export interface ThemeState {
  readonly preference: ThemePreference;
  readonly systemTheme: ThemeName | null;
  readonly resolved: ThemeName;
  readonly setPreference: (preference: ThemePreference) => void;
  readonly setSystemTheme: (theme: ThemeName | null) => void;
}

/** An explicit choice wins; `system` follows the OS and falls back to dark. */
export function resolveTheme(
  preference: ThemePreference,
  systemTheme: ThemeName | null,
): ThemeName {
  return preference === "system" ? (systemTheme ?? DEFAULT_THEME) : preference;
}

/**
 * Theme choice is ephemeral shell state, which is why it lives in Zustand rather
 * than in a component (docs/ARCHITECTURE.md §6). It moves into the settings
 * registry, and becomes persistent, with PG-E20.
 */
export const themeStore = createStore<ThemeState>((set, get) => ({
  preference: "system",
  systemTheme: null,
  resolved: DEFAULT_THEME,
  setPreference: (preference) => {
    set({ preference, resolved: resolveTheme(preference, get().systemTheme) });
  },
  setSystemTheme: (theme) => {
    set({ systemTheme: theme, resolved: resolveTheme(get().preference, theme) });
  },
}));
