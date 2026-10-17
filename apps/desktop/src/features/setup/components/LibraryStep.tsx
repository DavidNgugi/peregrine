import type { LibraryRootDto } from "@peregrine/ipc-client";
import { Button, ErrorState, Panel } from "@peregrine/ui";
import type { ReactElement } from "react";
import { useTranslation } from "react-i18next";
import { SetupStep } from "./SetupStep.js";

/** What the last scan attempt did, so the step can say so without a second source. */
export type SetupScanState = "idle" | "started" | "failed";

export interface LibraryStepProps {
  readonly roots: readonly LibraryRootDto[];
  readonly loading: boolean;
  readonly failed: boolean;
  readonly adding: boolean;
  /** A write is in flight, so the folder control is refused. */
  readonly busy: boolean;
  readonly error: string | null;
  readonly scan: SetupScanState;
  readonly onChoose: () => Promise<void>;
  readonly onScan: () => Promise<void>;
}

/**
 * One folder, then a scan.
 *
 * The scan starts as soon as the root is added, because choosing a folder and then having to
 * choose again to scan is two decisions for one intention. A scan that could not start leaves
 * the root in place and offers the retry here, rather than pretending the folder was not added.
 */
export function LibraryStep({
  roots,
  loading,
  failed,
  adding,
  busy,
  error,
  scan,
  onChoose,
  onScan,
}: LibraryStepProps): ReactElement {
  const { t } = useTranslation();

  return (
    <SetupStep body={t("setup.library.body")} title={t("setup.library.title")}>
      {failed ? (
        <ErrorState
          body={t("setup.library.unavailable.body")}
          title={t("setup.library.unavailable.title")}
        />
      ) : null}

      {!failed && error !== null && scan !== "failed" ? (
        <ErrorState body={error} title={t("setup.library.refused.title")} />
      ) : null}

      {loading ? (
        <p className="text-body text-text-secondary" role="status">
          {t("setup.library.loading")}
        </p>
      ) : roots.length === 0 ? (
        <p className="text-body text-text-tertiary">{t("setup.library.empty")}</p>
      ) : (
        <Panel className="flex flex-col divide-y divide-border-subtle overflow-hidden">
          {roots.map((root) => (
            <p className="truncate px-3 py-2 text-body text-text-primary" key={root.id}>
              {root.path}
            </p>
          ))}
        </Panel>
      )}

      <Button
        className="self-start"
        disabled={busy}
        onClick={() => {
          void onChoose();
        }}
        variant={roots.length === 0 ? "primary" : "secondary"}
      >
        {adding
          ? t("setup.library.adding")
          : roots.length === 0
            ? t("setup.library.choose")
            : t("setup.library.another")}
      </Button>

      {scan === "started" ? (
        <p className="text-body text-text-secondary" role="status">
          {t("setup.library.scanStarted")}
        </p>
      ) : null}

      {scan === "failed" ? (
        <ErrorState
          action={
            <Button
              disabled={busy}
              onClick={() => {
                void onScan();
              }}
              size="sm"
              variant="secondary"
            >
              {t("setup.library.scanFailed.action")}
            </Button>
          }
          body={error ?? t("setup.library.scanFailed.body")}
          title={t("setup.library.scanFailed.title")}
        />
      ) : null}
    </SetupStep>
  );
}
