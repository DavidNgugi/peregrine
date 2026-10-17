/**
 * The wizard's five steps, and the two rules that decide whether a step can be left.
 *
 * The order is a decision of its own, not a list of screens: the provider comes before the
 * indexers because it is the key most people already have, the library comes after both because
 * a folder is usable on its own and gives the last step something to summarise, and Done is the
 * only step whose primary action leaves the wizard.
 *
 * **A step that cannot be completed can be skipped only when the loss is survivable.** Provider
 * and indexers are skippable — the library still scans and plays without them — while the library
 * is not: an installation with no root has nothing to open, so its `Next` waits for a folder
 * instead of offering a way past it. `skipLabelKey` is that rule as data, so the footer cannot
 * grow a skip on a step that has no safe answer.
 *
 * **There is no account step, deliberately.** PG-E26-11's acceptance names a first account and an
 * avatar, but the account backend does not exist yet (PG-E20-11), and a step that collected a name
 * and threw it away would be a promise the application cannot keep. It lands with the backend, not
 * as a placeholder here.
 */

export type SetupStepId = "welcome" | "provider" | "indexers" | "library" | "done";

/** The steps in the order the wizard walks them. */
export const SETUP_STEPS = [
  "welcome",
  "provider",
  "indexers",
  "library",
  "done",
] as const satisfies readonly SetupStepId[];

/** Where a step sits in the order, zero-based. */
export function stepIndex(step: SetupStepId): number {
  return SETUP_STEPS.indexOf(step);
}

/** The step after this one, or `null` on Done. */
export function nextStep(step: SetupStepId): SetupStepId | null {
  return SETUP_STEPS[stepIndex(step) + 1] ?? null;
}

/** The step before this one, or `null` on Welcome. */
export function previousStep(step: SetupStepId): SetupStepId | null {
  const index = stepIndex(step);
  return index === 0 ? null : (SETUP_STEPS[index - 1] ?? null);
}

/**
 * Where Escape goes: one step back, except on Welcome, where it goes nowhere.
 *
 * The wizard is the whole window rather than a dialog over the shell, so there is nothing behind
 * it to dismiss to. Escape that closed it would leave the user on a shell whose library is empty
 * and whose only route back to the wizard is a restart.
 */
export function escapeTarget(step: SetupStepId): SetupStepId | null {
  return previousStep(step);
}

/** What the wizard has learned about the installation so far. */
export interface SetupFacts {
  /** A provider is configured, or this build has none to configure. */
  readonly providerReady: boolean;
  /** Prowlarr is running, this platform has no managed build, or the step was skipped. */
  readonly indexersReady: boolean;
  /** At least one library root exists. */
  readonly libraryReady: boolean;
}

/** Whether the step's primary action may leave it. */
export function canContinue(step: SetupStepId, facts: SetupFacts): boolean {
  switch (step) {
    case "welcome":
    case "done":
      return true;
    case "provider":
      return facts.providerReady;
    case "indexers":
      return facts.indexersReady;
    case "library":
      return facts.libraryReady;
  }
}

/**
 * The sentence on a step's skip control, or `null` where skipping is not safe.
 *
 * Welcome and Done have nothing to skip; the library's absence is what the wizard exists to fix,
 * so it has no skip at all.
 */
export function skipLabelKey(step: SetupStepId): string | null {
  switch (step) {
    case "provider":
      return "setup.provider.skip";
    case "indexers":
      return "setup.indexers.skip";
    case "welcome":
    case "library":
    case "done":
      return null;
  }
}

/** Why the primary action is unavailable, or `null` when it is available. */
export function continueHintKey(step: SetupStepId): string | null {
  switch (step) {
    case "provider":
      return "setup.provider.required";
    case "indexers":
      return "setup.indexers.required";
    case "library":
      return "setup.library.required";
    case "welcome":
    case "done":
      return null;
  }
}
