import { ErrorState, SettingsRow, SettingsSection } from "@peregrine/ui";
import type { ReactElement } from "react";
import { useTranslation } from "react-i18next";

import { useAbout } from "../hooks/useAbout.js";

/**
 * What this build is, and the notices it owes.
 *
 * The two reads have no runtime behind them (`PG-E5-19`), so this section answers on an
 * installation with no provider configured and no key stored. The logo TMDB's terms also require
 * is deliberately admitted rather than faked: the manifest says this build does not ship it yet,
 * and a screen that stayed silent would be the one place the gap is invisible.
 */
export function AboutSection(): ReactElement {
  const { t } = useTranslation();
  const about = useAbout();

  return (
    <SettingsSection
      description={t("settings.about.description")}
      title={t("settings.about.title")}
    >
      {about.failed ? <ErrorState title={t("settings.about.failed")} /> : null}
      {about.build === null ? (
        about.failed ? null : (
          <p className="px-3 py-2 text-body text-text-secondary" role="status">
            {t("settings.about.loading")}
          </p>
        )
      ) : (
        <>
          <SettingsRow label={t("settings.about.version")}>
            <span className="text-body text-text-primary">
              {t("settings.about.value", {
                name: about.build.appName,
                version: about.build.appVersion,
              })}
            </span>
          </SettingsRow>
          <SettingsRow label={t("settings.about.target")}>
            <span className="text-body text-text-primary">{about.build.target}</span>
          </SettingsRow>
        </>
      )}

      {about.attribution.length === 0 ? null : (
        <div className="flex flex-col gap-3 px-3 py-2">
          {about.attribution.map((entry) => (
            <div className="flex flex-col gap-1" key={entry.provider}>
              <span className="text-body text-text-primary">{entry.provider}</span>
              <p className="text-caption text-text-secondary">{entry.notice}</p>
              {entry.requiresLogo ? (
                <p className="text-caption text-text-tertiary">
                  {t("settings.about.logoRequired", { provider: entry.provider })}
                </p>
              ) : null}
            </div>
          ))}
        </div>
      )}

      <SettingsRow label={t("settings.about.licences")}>
        <span className="max-w-xs text-right text-caption text-text-secondary">
          {t("settings.about.licencesBody")}
        </span>
      </SettingsRow>
    </SettingsSection>
  );
}
