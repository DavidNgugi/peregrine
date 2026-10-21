/**
 * The managed Prowlarr section of the settings screen (docs/adr/0024).
 *
 * This is the zero-setup path: the application downloads, verifies, and runs a local Prowlarr, then
 * reads the API key Prowlarr generated and stores it. The base-URL/API-key section is the custom
 * path — a remote, homelab, or future media-server instance — and stays available either way.
 */

import { Button, Dialog, ErrorState, Progress, SettingsRow, SettingsSection } from "@peregrine/ui";
import type { ReactElement } from "react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { ManagedProwlarr } from "../hooks/useProwlarr.js";

interface ManagedProwlarrSectionProps {
  /** The managed instance and the operations this section offers. */
  readonly prowlarr: ManagedProwlarr;
}

/** The managed Prowlarr section of the settings screen. */
export function ManagedProwlarrSection({ prowlarr }: ManagedProwlarrSectionProps): ReactElement {
  const { t } = useTranslation();
  const status = prowlarr.status;
  const [confirmingStop, setConfirmingStop] = useState(false);

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
          <div className="mt-2 flex flex-wrap items-center justify-end gap-2">
            {status.installed ? null : (
              <Button
                busy={prowlarr.phase === "installing"}
                busyLabel={t("settings.prowlarr.installing")}
                disabled={prowlarr.busy}
                onClick={() => prowlarr.install()}
                progress={prowlarr.progressPermille / 10}
                variant="primary"
              >
                {prowlarr.phase === "installing"
                  ? t("settings.prowlarr.installing")
                  : t("settings.prowlarr.install")}
              </Button>
            )}
            {status.installed && !status.running ? (
              <Button
                busy={prowlarr.phase === "starting"}
                busyLabel={t("settings.prowlarr.starting")}
                disabled={prowlarr.busy}
                onClick={() => prowlarr.start()}
                variant="primary"
              >
                {prowlarr.phase === "starting"
                  ? t("settings.prowlarr.starting")
                  : t("settings.prowlarr.start")}
              </Button>
            ) : null}
            {status.running ? (
              <Button
                disabled={prowlarr.busy}
                onClick={() => {
                  setConfirmingStop(true);
                }}
                variant="danger"
              >
                {t("settings.prowlarr.stop")}
              </Button>
            ) : null}
          </div>
          {confirmingStop ? (
            <Dialog
              closeLabel={t("settings.prowlarr.stopConfirm.cancel")}
              description={t("settings.prowlarr.stopConfirm.body")}
              onClose={() => {
                setConfirmingStop(false);
              }}
              title={t("settings.prowlarr.stopConfirm.title")}
            >
              <div className="flex justify-end gap-2">
                <Button
                  onClick={() => {
                    setConfirmingStop(false);
                  }}
                  variant="ghost"
                >
                  {t("settings.prowlarr.stopConfirm.cancel")}
                </Button>
                <Button
                  onClick={() => {
                    setConfirmingStop(false);
                    void prowlarr.stop();
                  }}
                  variant="danger"
                >
                  {t("settings.prowlarr.stopConfirm.confirm")}
                </Button>
              </div>
            </Dialog>
          ) : null}
          {prowlarr.phase === "installing" ? (
            <Progress
              label={t("settings.prowlarr.installing")}
              value={Math.min(100, Math.max(0, prowlarr.progressPermille / 10))}
            />
          ) : null}
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
