import type { ReactElement } from "react";
import { useTranslation } from "react-i18next";

/**
 * The Discover screen, which is a shell until PG-E7-01 lands: it states what the
 * screen is and that it has no data, and computes nothing.
 */
export function DiscoverScreen(): ReactElement {
  const { t } = useTranslation();

  return (
    <section
      aria-labelledby="discover-title"
      className="flex h-full flex-col items-center justify-center gap-2 px-6 text-center"
    >
      <h1 className="text-title-1 text-text-primary" id="discover-title">
        {t("discover.title")}
      </h1>
      <p className="text-body-strong text-text-secondary">{t("discover.empty.title")}</p>
      <p className="max-w-md text-body text-text-tertiary">{t("discover.empty.body")}</p>
    </section>
  );
}
