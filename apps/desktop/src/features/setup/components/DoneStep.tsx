import { Panel } from "@peregrine/ui";
import type { ReactElement } from "react";
import { useTranslation } from "react-i18next";
import { SetupStep } from "./SetupStep.js";

/** What the search line can say: running, impossible on this platform, or skipped. */
export type SetupIndexerSummary = "ready" | "unsupported" | "skipped";

export interface DoneStepProps {
  readonly providerConfigured: boolean;
  readonly indexers: SetupIndexerSummary;
  readonly libraryCount: number;
}

const INDEXER_KEY: Readonly<Record<SetupIndexerSummary, string>> = {
  ready: "setup.done.indexers.ready",
  unsupported: "setup.done.indexers.unsupported",
  skipped: "setup.done.indexers.skipped",
};

/** What the wizard changed, and the primary action that leaves it. */
export function DoneStep({
  providerConfigured,
  indexers,
  libraryCount,
}: DoneStepProps): ReactElement {
  const { t } = useTranslation();

  return (
    <SetupStep body={t("setup.done.body")} title={t("setup.done.title")}>
      <Panel className="flex flex-col divide-y divide-border-subtle overflow-hidden">
        <p className="px-3 py-2 text-body text-text-primary">
          {providerConfigured
            ? t("setup.done.provider.configured")
            : t("setup.done.provider.skipped")}
        </p>
        <p className="px-3 py-2 text-body text-text-primary">{t(INDEXER_KEY[indexers])}</p>
        <p className="px-3 py-2 text-body text-text-primary">
          {t("setup.done.library", { count: libraryCount })}
        </p>
      </Panel>
    </SetupStep>
  );
}
