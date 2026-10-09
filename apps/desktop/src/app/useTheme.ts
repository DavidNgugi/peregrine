import type { ThemeName } from "@peregrine/theme";
import { useEffect } from "react";
import { useStore } from "zustand";
import { applyTheme, readOsTheme } from "../lib/theme-runtime.js";
import { themeStore, type ThemePreference } from "./theme-store.js";

export interface ThemeControls {
  readonly resolved: ThemeName;
  readonly setPreference: (preference: ThemePreference) => void;
}

/** Applies the resolved theme to the document and follows the OS once at startup. */
export function useTheme(): ThemeControls {
  const resolved = useStore(themeStore, (state) => state.resolved);
  const setPreference = useStore(themeStore, (state) => state.setPreference);

  useEffect(() => {
    applyTheme(resolved);
  }, [resolved]);

  useEffect(() => {
    let cancelled = false;
    void readOsTheme().then((theme) => {
      if (!cancelled) {
        themeStore.getState().setSystemTheme(theme);
      }
    });
    return () => {
      cancelled = true;
    };
  }, []);

  return { resolved, setPreference };
}
