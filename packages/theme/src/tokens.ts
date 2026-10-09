/**
 * The token model behind `tokens.css` (docs/DESIGN_SYSTEM.md §2.1 and §3).
 *
 * The stylesheet holds the values; this module holds the names, so the runtime,
 * the tests, and the future `xtask check-theme` all agree on one vocabulary.
 */

/** The themes the design system ships. Dark is the default. */
export type ThemeName = "dark" | "light";

/** Attribute the runtime sets on `<html>`; `tokens.css` selects on it. */
export const THEME_ATTRIBUTE = "data-theme";

export const DEFAULT_THEME: ThemeName = "dark";

/** Narrows an untrusted string — a Tauri `theme()` result, for example. */
export function isThemeName(value: unknown): value is ThemeName {
  return value === "dark" || value === "light";
}

/** The single custom property the whole accent family is derived from. */
export const BRAND_HUE_TOKEN = "--hue-brand";

/** Semantic colour layer (§2.1). Components use these and nothing else. */
export const SEMANTIC_COLOUR_TOKENS = [
  "--surface-0",
  "--surface-1",
  "--surface-2",
  "--surface-scrim",
  "--text-primary",
  "--text-secondary",
  "--text-tertiary",
  "--text-on-accent",
  "--border-subtle",
  "--border-strong",
  "--accent",
  "--accent-hover",
  "--accent-muted",
  "--state-ok",
  "--state-warn",
  "--state-danger",
  "--state-info",
  "--focus-ring",
] as const;

/** Colour tokens the light theme overrides; the rest are inherited from `:root`. */
export const LIGHT_THEME_OVERRIDES = [
  "--surface-0",
  "--surface-1",
  "--surface-2",
  "--text-primary",
  "--text-secondary",
  "--text-tertiary",
  "--border-subtle",
  "--accent",
] as const;

export const TYPOGRAPHY_FONT_TOKENS = ["--font-ui"] as const;

/** Text tokens that carry a `--line-height` companion (§3). */
export const TYPOGRAPHY_SIZE_TOKENS = [
  "--text-display",
  "--text-title-1",
  "--text-title-2",
  "--text-body",
  "--text-body-strong",
  "--text-caption",
  "--text-micro",
] as const;
