/**
 * The managed Prowlarr section of the settings screen (docs/adr/0024).
 *
 * This is the zero-setup path: the application downloads, verifies, and runs a local Prowlarr, then
 * reads the API key Prowlarr generated and stores it. The base-URL/API-key section is the custom
 * path — a remote, homelab, or future media-server instance — and stays available either way.
 */

import { Button, ErrorState, SettingsRow, SettingsSection } from "@peregrine/ui";
import type { ReactElement } from "react";
import { useTranslation } from "react-i18next";
import type { ManagedProwlarr } from "../hooks/useProwlarr.js";

interface ManagedProwlarrSectionProps {
  /** The managed instance and the operations this section offers. */
  readonly prowlarr: ManagedProwlarr;
}

/** The managed Prowlarr section of the settings screen. */
export function ManagedProwlarrSection({
  prowlarr,
}: ManagedProwlarrSectionProps): ReactElement {
  const { t } = useTranslation();
  const status = prowlarr.status;

  return (
    <SettingsSection
      description={t("settings.prowlarr.description")}
      title={t("settings.prowlarr.title")}
    >
      {prowlarr.failed ? (
        <ErrorState
          body={t("settings.prowlarr.unavailable.body")}
          title={t("settings.prowlarr.unavailable.title")}
        />
      ) : status === null ? null : !status.supported ? (
        <p className="text-body text-text-secondary">{t("settings.prowlarr.unsupported")}</p>
      ) : (
        <>
          <SettingsRow label={t("settings.prowlarr.state")}>
            <span>
              {status.running
                ? t("settings.prowlarr.runningAt", { url: status.baseUrl ?? "" })
                : status.installed
                  ? t("settings.prowlarr.installed")
                  : t("settings.prowlarr.notInstalled")}
            </span>
          </SettingsRow>
          <SettingsRow label={t("settings.prowlarr.version")}>
            <span>{status.version}</span>
          </SettingsRow>
          <div className="flex flex-wrap items-center gap-3">
            {status.installed ? null : (
              <Button
                disabled={prowlarr.busy}
                onClick={() => {
                  void prowlarr.install();
                }}
                variant="primary"
              >
                {t("settings.prowlarr.install")}
              </Button>
            )}
            {status.installed && !status.running ? (
              <Button
                disabled={prowlarr.busy}
                onClick={() => {
                  void prowlarr.start();
                }}
                variant="primary"
              >
                {t("settings.prowlarr.start")}
              </Button>
            ) : null}
            {status.running ? (
              <Button
                disabled={prowlarr.busy}
                onClick={() => {
                  void prowlarr.stop();
                }}
                variant="secondary"
              >
                {t("settings.prowlarr.stop")}
              </Button>
            ) : null}
          </div>
          {prowlarr.error !== null ? (
            <p className="text-caption text-text-secondary" role="status">
              {prowlarr.error}
            </p>
          ) : prowlarr.outcome !== null ? (
            <p className="text-caption text-text-secondary" role="status">
              {t(`settings.prowlarr.outcome.${prowlarr.outcome}`)}
            </p>
          ) : null}
          <p className="text-caption text-text-tertiary">{t("settings.prowlarr.customHint")}</p>
        </>
      )}
    </SettingsSection>
  );
}
