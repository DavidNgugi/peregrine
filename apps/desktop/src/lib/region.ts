/**
 * The region a screen asks about before anyone has chosen one.
 *
 * The webview already knows the operating system's locale, and its last segment is either a region
 * ("en-KE" → KE) or a script ("zh-Hant" → Hant, refused). A locale without a region falls back to
 * US rather than asking the provider about a code that is not one. This is a display default, not
 * a setting: the settings registry (PG-E20-01) is where a reader will be able to pin one.
 */
export function regionFromLocale(locale: string, fallback = "US"): string {
  const parts = locale.split(/[-_]/);
  // A bare two-letter locale is a language ("en"), not a region: only the segment after a
  // separator can name a country.
  const candidate = parts.length > 1 ? (parts.at(-1)?.toUpperCase() ?? "") : "";
  return /^[A-Z]{2}$/.test(candidate) ? candidate : fallback;
}

/** The current region, from the webview's locale, with the same fallback. */
export function currentRegion(): string {
  const locale = typeof navigator === "undefined" ? "" : navigator.language;
  return regionFromLocale(locale);
}
