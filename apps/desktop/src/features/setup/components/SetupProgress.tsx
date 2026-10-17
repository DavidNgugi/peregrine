import type { ReactElement } from "react";
import { useTranslation } from "react-i18next";
import { SETUP_STEPS, stepIndex, type SetupStepId } from "../setup-steps.js";

interface SetupProgressProps {
  readonly current: SetupStepId;
}

/**
 * Where the wizard is, as five segments and a count.
 *
 * The segments are decorative; the sentence "Step 2 of 5" and the step names beside each segment
 * carry the meaning for assistive technology. `aria-current="step"` is on the list item rather
 * than the segment so a screen reader announces the current step once, with its name.
 */
export function SetupProgress({ current }: SetupProgressProps): ReactElement {
  const { t } = useTranslation();
  const index = stepIndex(current);

  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center justify-between gap-4">
        <p className="text-caption text-text-tertiary">{t("setup.label")}</p>
        <p className="text-caption text-text-secondary">
          {t("setup.progress.stepOf", { current: index + 1, total: SETUP_STEPS.length })}
        </p>
      </div>
      <nav aria-label={t("setup.progress.label")}>
        <ol className="flex items-center gap-1.5">
          {SETUP_STEPS.map((step, position) => (
            <li
              aria-current={step === current ? "step" : undefined}
              className="min-w-0 flex-1"
              key={step}
            >
              <span
                className={
                  position <= index
                    ? "block h-1 rounded-full bg-accent"
                    : "block h-1 rounded-full bg-surface-2"
                }
              />
              <span className="sr-only">{t(`setup.progress.steps.${step}`)}</span>
            </li>
          ))}
        </ol>
      </nav>
    </div>
  );
}
