import type { ReactElement } from "react";
import { useTranslation } from "react-i18next";
import { SettingsSection } from "@peregrine/ui";

/**
 * What this build does and does not do about a VPN.
 *
 * **No toggle, deliberately.** A tunnel managed here would be a promise of leak safety that this
 * build cannot keep across three operating systems, and a half-working kill switch is worse than
 * the operating system's own. The section tells the reader how to run the tunnel they already
 * have and how to check that it is carrying traffic.
 */
export function NetworkSection(): ReactElement {
  const { t } = useTranslation();

  return (
    <SettingsSection
      description={t("settings.network.description")}
      title={t("settings.network.title")}
    >
      <div className="flex max-w-prose flex-col gap-3 text-body text-text-secondary">
        <p>{t("settings.network.body")}</p>
        <p>{t("settings.network.verify")}</p>
        <p className="text-state-warn">{t("settings.network.killSwitch")}</p>
        <p className="text-caption text-text-tertiary">{t("settings.network.binding")}</p>
      </div>
    </SettingsSection>
  );
}
