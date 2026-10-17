import { normalizeIpcError, prowlarrStatus } from "@peregrine/ipc-client";
import type { ReactElement } from "react";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { logUiError } from "../../../lib/error-log.js";
import { messageOf } from "../../../lib/ipc-message.js";
import { useLibraryRoots, useProviders, useProwlarr } from "../../settings/index.js";
import {
  escapeTarget,
  nextStep,
  previousStep,
  type SetupFacts,
  type SetupStepId,
} from "../setup-steps.js";
import { DoneStep, type SetupIndexerSummary } from "./DoneStep.js";
import { IndexersStep } from "./IndexersStep.js";
import { LibraryStep, type SetupScanState } from "./LibraryStep.js";
import { ProviderStep } from "./ProviderStep.js";
import { SetupFooter } from "./SetupFooter.js";
import { SetupProgress } from "./SetupProgress.js";
import { WelcomeStep } from "./WelcomeStep.js";

export interface SetupWizardProps {
  /** The user pressed "Open Peregrine" on the last step. */
  readonly onFinished: () => void;
}

/**
 * The first-run wizard (PG-E26-11), as five screens that each ask one question.
 *
 * **It composes the same reads and writes the Settings screen uses.** The provider job, the
 * managed Prowlarr and the library roots are already implemented as hooks there, including the
 * job-following that turns a started job into a sentence. A second copy of that plumbing here
 * would be a second answer to what a failed key means; the wizard supplies the sequence and the
 * copy, and the features' public hooks supply the facts.
 *
 * **The wizard is the whole window, so nothing here scrolls.** The frame is a fixed
 * header/content/footer column, every step paints its heading before any read answers, and the
 * content is clipped rather than scrolled — a setup decision that needs a scrollbar is a decision
 * that needs splitting. Escape is Back, and Back on Welcome does nothing rather than dismissing
 * into an empty shell ({@link escapeTarget}).
 */
export function SetupWizard({ onFinished }: SetupWizardProps): ReactElement {
  const { t } = useTranslation();
  const providers = useProviders();
  const prowlarr = useProwlarr();
  const libraries = useLibraryRoots();
  const [step, setStep] = useState<SetupStepId>("welcome");
  const [adding, setAdding] = useState(false);
  const [scan, setScan] = useState<SetupScanState>("idle");
  const [localError, setLocalError] = useState<string | null>(null);
  const surface = useRef<HTMLDivElement | null>(null);

  const provider =
    providers.providers.find((entry) => entry.configured) ?? providers.providers[0] ?? null;

  // A provider read that has not answered is not "no provider": the Next control waits, and the
  // step's skip is the safe way past it while the keychain is still being read. A read that
  // *failed* is different again — there is nothing left to configure in this visit, so the step
  // reports the failure and lets the user continue to Settings later.
  const providerReady = providers.loading
    ? false
    : providers.failed
      ? true
      : provider === null || provider.configured;
  const indexersReady =
    prowlarr.status !== null && (prowlarr.status.supported === false || prowlarr.status.running);
  // "Can leave the step" and "search works" are different facts on a platform with no managed
  // build: the step must let the user through, and the summary must not claim a search that
  // cannot exist there.
  const indexerSummary: SetupIndexerSummary =
    prowlarr.status?.running === true
      ? "ready"
      : prowlarr.status?.supported === false
        ? "unsupported"
        : "skipped";
  const facts: SetupFacts = {
    providerReady,
    indexersReady,
    libraryReady: libraries.roots.length > 0,
  };

  // The wizard moves focus to the new step's heading, so a keyboard or screen-reader user is told
  // what the step is before they are asked for anything. The heading is `tabIndex={-1}`, so this
  // is a move within the step rather than another tab stop.
  useEffect(() => {
    surface.current?.querySelector<HTMLElement>("h1")?.focus();
  }, [step]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.key !== "Escape" || event.defaultPrevented) {
        return;
      }
      const target = escapeTarget(step);
      if (target === null) {
        return;
      }
      event.preventDefault();
      setStep(target);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [step]);

  const goBack = useCallback(() => {
    const target = previousStep(step);
    if (target !== null) {
      setStep(target);
    }
  }, [step]);

  const goNext = useCallback(() => {
    if (step === "done") {
      onFinished();
      return;
    }
    const target = nextStep(step);
    if (target !== null) {
      setStep(target);
    }
  }, [onFinished, step]);

  const skip = useCallback(() => {
    const target = nextStep(step);
    if (target !== null) {
      setStep(target);
    }
  }, [step]);

  // The install job leaves the pack installed and the phase idle; the managed instance still has
  // to be started before it can answer a search. The chain is the wizard's rather than the
  // settings hook's because the settings screen offers the two as separate decisions, and this
  // step is deliberately one click.
  const installIndexers = useCallback(async () => {
    await prowlarr.install();
    const current = await prowlarrStatus().catch(() => null);
    if (current?.installed === true && !current.running) {
      await prowlarr.start();
    }
  }, [prowlarr.install, prowlarr.start]);

  const startIndexers = useCallback(async () => {
    await prowlarr.start();
  }, [prowlarr.start]);

  const chooseFolder = useCallback(async () => {
    setAdding(true);
    setLocalError(null);
    try {
      const added = await libraries.add("mixed");
      // The folder is chosen; what remains is starting the scan, and the button must stop saying
      // "Adding the folder…" the moment that is no longer what it is doing.
      setAdding(false);
      if (added === null) {
        return;
      }
      const started = await libraries.scan();
      setScan(started === null ? "failed" : "started");
    } catch (caught: unknown) {
      // The hook records its own refusals; this is the picker itself failing to open, which the
      // hook does not see because it awaits the dialog before its own try block.
      const error = normalizeIpcError(caught);
      logUiError("setup.library.add", error);
      setLocalError(messageOf(error));
    } finally {
      setAdding(false);
    }
  }, [libraries.add, libraries.scan]);

  const scanAgain = useCallback(async () => {
    const started = await libraries.scan();
    setScan(started === null ? "failed" : "started");
  }, [libraries.scan]);

  return (
    <div
      className="flex h-full min-h-0 flex-col overflow-hidden bg-surface-0 text-text-primary"
      data-setup-frame=""
    >
      <header className="shrink-0 px-8 pt-6">
        <SetupProgress current={step} />
      </header>
      {/* `overflow-hidden` and `min-h-0` are the fixed viewport: the content area takes the space
          left between the header and the footer and clips, rather than becoming a second scroller
          the user has to find. */}
      <main
        aria-label={t("setup.label")}
        className="flex min-h-0 flex-1 items-center justify-center overflow-hidden px-8 py-4"
      >
        <div className="w-full max-w-xl" ref={surface}>
          {step === "welcome" ? <WelcomeStep /> : null}
          {step === "provider" ? (
            <ProviderStep
              busy={providers.busy}
              error={providers.error}
              failed={providers.failed}
              loading={providers.loading}
              onRetryStore={providers.reload}
              onSave={providers.saveKey}
              onTest={providers.testConnection}
              outcome={providers.outcome}
              store={providers.store}
              view={
                provider === null
                  ? null
                  : {
                      provider: provider.provider,
                      displayName: provider.displayName,
                      configured: provider.configured,
                      // No build ships a key today; a build that does sets this and passes
                      // `onUseBuiltIn`, and the step becomes the one-click path.
                      builtIn: false,
                    }
              }
            />
          ) : null}
          {step === "indexers" ? (
            <IndexersStep
              busy={prowlarr.busy}
              error={prowlarr.error}
              failed={prowlarr.failed}
              loading={prowlarr.loading}
              onInstall={installIndexers}
              onStart={startIndexers}
              phase={prowlarr.phase}
              progressPermille={prowlarr.progressPermille}
              status={prowlarr.status}
            />
          ) : null}
          {step === "library" ? (
            <LibraryStep
              adding={adding}
              busy={libraries.busy}
              error={localError ?? libraries.error}
              failed={libraries.failed}
              loading={libraries.loading}
              onChoose={chooseFolder}
              onScan={scanAgain}
              roots={libraries.roots}
              scan={scan}
            />
          ) : null}
          {step === "done" ? (
            <DoneStep
              indexers={indexerSummary}
              libraryCount={libraries.roots.length}
              providerConfigured={provider?.configured ?? false}
            />
          ) : null}
        </div>
      </main>
      <SetupFooter facts={facts} onBack={goBack} onNext={goNext} onSkip={skip} step={step} />
    </div>
  );
}
