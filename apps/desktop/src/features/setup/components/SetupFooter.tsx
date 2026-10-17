import { Button } from "@peregrine/ui";
import type { ReactElement } from "react";
import { useTranslation } from "react-i18next";
import {
  canContinue,
  continueHintKey,
  skipLabelKey,
  type SetupFacts,
  type SetupStepId,
} from "../setup-steps.js";

export interface SetupFooterProps {
  readonly step: SetupStepId;
  readonly facts: SetupFacts;
  readonly onBack: () => void;
  readonly onNext: () => void;
  readonly onSkip: () => void;
}

/**
 * Back, the step's own skip, and the primary action.
 *
 * The skip is drawn only while the step cannot be completed, and only on the steps
 * {@link skipLabelKey} names. Once the provider is configured or Prowlarr is running there is
 * nothing to skip past, and the library never offers one: its absence is the thing the wizard
 * exists to fix.
 */
export function SetupFooter({
  step,
  facts,
  onBack,
  onNext,
  onSkip,
}: SetupFooterProps): ReactElement {
  const { t } = useTranslation();
  const ready = canContinue(step, facts);
  const skipKey = ready ? null : skipLabelKey(step);
  const hintKey = ready ? null : continueHintKey(step);
  const nextLabel =
    step === "welcome"
      ? t("setup.welcome.start")
      : step === "done"
        ? t("setup.done.open")
        : t("setup.next");

  return (
    <footer className="flex shrink-0 flex-col gap-2 border-t border-border-subtle px-8 py-4">
      {hintKey === null ? null : (
        <p className="text-caption text-text-tertiary" role="status">
          {t(hintKey)}
        </p>
      )}
      <div className="flex items-center justify-between gap-3">
        <Button disabled={step === "welcome"} onClick={onBack} variant="ghost">
          {t("setup.back")}
        </Button>
        <div className="flex items-center gap-2">
          {skipKey === null ? null : (
            <Button onClick={onSkip} variant="ghost">
              {t(skipKey)}
            </Button>
          )}
          <Button disabled={!ready} onClick={onNext} variant="primary">
            {nextLabel}
          </Button>
        </div>
      </div>
    </footer>
  );
}
