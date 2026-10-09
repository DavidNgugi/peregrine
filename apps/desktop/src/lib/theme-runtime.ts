import { normalizeIpcError } from "@peregrine/ipc-client";
import { isThemeName, THEME_ATTRIBUTE, type ThemeName } from "@peregrine/theme";
import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { logUiError } from "./error-log.js";

/** Points the document at a theme; `tokens.css` selects on this attribute. */
export function applyTheme(theme: ThemeName): void {
  document.documentElement.setAttribute(THEME_ATTRIBUTE, theme);
}

/**
 * The OS appearance, or `null` when it cannot be known — outside the Tauri shell,
 * or when the window call fails.
 *
 * A failed read must never stop the shell from starting (docs/ARCHITECTURE.md
 * §15), so it degrades to the default theme and leaves a log line behind.
 */
export async function readOsTheme(): Promise<ThemeName | null> {
  if (!isTauri()) {
    return null;
  }
  try {
    const theme = await getCurrentWindow().theme();
    return isThemeName(theme) ? theme : null;
  } catch (caught) {
    logUiError("readOsTheme", normalizeIpcError(caught));
    return null;
  }
}
