import { useEffect, type ReactElement } from "react";
import { useTranslation } from "react-i18next";
import { Button, SettingsSection } from "@peregrine/ui";
import { usePublicIp } from "../hooks/usePublicIp.js";

/**
 * What this build does and does not do about a VPN.
 *
 * **No toggle, deliberately.** A tunnel managed here would be a promise of leak safety that this
 * build cannot keep across three operating systems, and a half-working kill switch is worse than
 * the operating system's own. The section tells the reader how to run the tunnel they already
 * have and how to check that it is carrying traffic.
 */
/// One automatic read per app run, not per mount: StrictMode's second mount and a return visit
/// both find this set, and the button is how a reader asks again after changing tunnels.
let autoChecked = false;

export function NetworkSection(): ReactElement {
  const { t } = useTranslation();
  const check = usePublicIp();
  useEffect(() => {
    if (autoChecked) {
      return;
    }
    autoChecked = true;
    void check.check();
  }, [check.check]);

  return (
    <SettingsSection
      description={t("settings.network.description")}
      title={t("settings.network.title")}
    >
      <div className="flex w-full max-w-prose flex-col gap-3 px-3 py-2 text-body text-text-secondary">
        <p>{t("settings.network.body")}</p>
        <p>{t("settings.network.verify")}</p>
        <div className="flex flex-wrap items-center gap-3">
          <Button
            disabled={check.checking}
            onClick={() => {
              void check.check();
            }}
            size="sm"
            variant="secondary"
          >
            {check.checking ? t("settings.network.checking") : t("settings.network.check")}
          </Button>
          {check.ip === null ? null : (
            <p className="text-body text-text-primary" role="status">
              {t("settings.network.result", { ip: check.ip })}
            </p>
          )}
          {check.failed ? (
            <p className="text-body text-state-danger" role="alert">
              {t("settings.network.failed")}
            </p>
          ) : null}
        </div>
        <p className="text-state-warn">{t("settings.network.killSwitch")}</p>
        <p className="text-caption text-text-tertiary">{t("settings.network.binding")}</p>
      </div>
    </SettingsSection>
  );
}
