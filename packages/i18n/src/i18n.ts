import { createInstance, type i18n as I18nInstance } from "i18next";
import { CATALOGUES } from "./catalogues.js";
import { DEFAULT_LOCALE, type SupportedLocale } from "./locales.js";

/**
 * Builds an isolated i18next instance for one locale.
 *
 * The app injects it with `<I18nextProvider>`, which keeps the global singleton
 * free for tests. Messages are bundled, so `initAsync: false` makes init complete
 * within this call and the first render already has copy; there is no backend and
 * therefore no asynchronous failure to report.
 */
export function createI18n(locale: SupportedLocale = DEFAULT_LOCALE): I18nInstance {
  const instance = createInstance();
  void instance.init({
    initAsync: false,
    lng: locale,
    fallbackLng: DEFAULT_LOCALE,
    resources: CATALOGUES,
    // React escapes text nodes, so escaping here would only render entity text.
    interpolation: { escapeValue: false },
  });
  return instance;
}
