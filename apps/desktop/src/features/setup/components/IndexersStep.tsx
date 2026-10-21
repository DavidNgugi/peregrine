import type { ProwlarrStatusDto } from "@peregrine/ipc-client";
import { Button, ErrorState, Progress } from "@peregrine/ui";
import type { ReactElement } from "react";
import { useTranslation } from "react-i18next";
import { SetupStep } from "./SetupStep.js";

export interface IndexersStepProps {
  readonly status: ProwlarrStatusDto | null;
  readonly loading: boolean;
  readonly failed: boolean;
  readonly busy: boolean;
  /** The effective phase: optimistic while an action begins, else the core's. */
  readonly phase: string;
  readonly progressPermille: number;
  readonly error: string | null;
  readonly onInstall: () => Promise<void>;
  readonly onStart: () => Promise<void>;
}

/**
 * One click for the managed instance (ADR 0024).
 *
 * The install is followed by a start when the pack is on disk but not running, because an
 * installed Prowlarr that is not running answers nothing: the step would claim search was set up
 * and then leave the user with an empty result. The wizard's install action owns that pair; this
 * component only says which of the two the button will do.
 */
export function IndexersStep({
  status,
  loading,
  failed,
  busy,
  phase,
  progressPermille,
  error,
  onInstall,
  onStart,
}: IndexersStepProps): ReactElement {
  const { t } = useTranslation();
  // The core can be installing or starting on its own (a launch, a second window); the button
  // follows the phase as well as this component's own busy flag, so a second action cannot be
  // started over one that is already running.
  const installing = status !== null && (phase === "installing" || (busy && !status.installed));
  const starting = status !== null && (phase === "starting" || (busy && status.installed));

  return (
    <SetupStep body={t("setup.indexers.body")} title={t("setup.indexers.title")}>
      {failed ? (
        <ErrorState
          body={t("setup.indexers.unavailable.body")}
          title={t("setup.indexers.unavailable.title")}
        />
      ) : status === null ? (
        loading ? (
          <p className="text-body text-text-secondary" role="status">
            {t("setup.indexers.loading")}
          </p>
        ) : null
      ) : !status.supported ? (
        <p className="text-body text-text-secondary">{t("setup.indexers.unsupported")}</p>
      ) : (
        <>
          {status.running ? (
            <p className="text-body text-text-secondary" role="status">
              {t("setup.indexers.running")}
            </p>
          ) : status.installed ? (
            <p className="text-body text-text-secondary" role="status">
              {t("setup.indexers.installed")}
            </p>
          ) : null}
          {phase === "installing" ? (
            <Progress label={t("setup.indexers.progress")} value={progressPermille / 10} />
          ) : null}
          {phase === "starting" ? (
            <p className="text-body text-text-secondary" role="status">
              {t("setup.indexers.starting")}
            </p>
          ) : null}
          {error === null ? null : <ErrorState title={error} />}
          {status.running ? null : (
            <Button
              className="self-start"
              disabled={installing || starting}
              onClick={() => (status.installed ? onStart() : onInstall())}
              variant="primary"
            >
              {installing
                ? t("setup.indexers.installing")
                : starting
                  ? t("setup.indexers.starting")
                  : t(status.installed ? "setup.indexers.start" : "setup.indexers.install")}
            </Button>
          )}
        </>
      )}
    </SetupStep>
  );
}
