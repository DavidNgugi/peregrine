/** Locales with a shipped catalogue. BCP 47 tags, lowercase, exact match only. */
export const SUPPORTED_LOCALES = ["en"] as const;

export type SupportedLocale = (typeof SUPPORTED_LOCALES)[number];

export const DEFAULT_LOCALE: SupportedLocale = "en";

/** Narrows a locale read from settings or the OS, which are both untrusted here. */
export function isSupportedLocale(value: unknown): value is SupportedLocale {
  return typeof value === "string" && SUPPORTED_LOCALES.some((locale) => locale === value);
}
