import { describe, expect, it } from "vitest";
import {
  canContinue,
  continueHintKey,
  escapeTarget,
  nextStep,
  previousStep,
  SETUP_STEPS,
  skipLabelKey,
  stepIndex,
  type SetupFacts,
  type SetupStepId,
} from "./setup-steps.js";

const READY: SetupFacts = {
  providerReady: true,
  indexersReady: true,
  libraryReady: true,
};

const NOTHING: SetupFacts = {
  providerReady: false,
  indexersReady: false,
  libraryReady: false,
};

describe("the step order", () => {
  it("walks welcome, provider, indexers, library, done", () => {
    expect(SETUP_STEPS).toEqual(["welcome", "provider", "indexers", "library", "done"]);
    expect(SETUP_STEPS.map((step) => stepIndex(step))).toEqual([0, 1, 2, 3, 4]);
  });

  it("moves forward until Done, which has nowhere after it", () => {
    expect(nextStep("welcome")).toBe("provider");
    expect(nextStep("provider")).toBe("indexers");
    expect(nextStep("indexers")).toBe("library");
    expect(nextStep("library")).toBe("done");
    expect(nextStep("done")).toBeNull();
  });

  it("moves back until Welcome, which has nowhere before it", () => {
    expect(previousStep("done")).toBe("library");
    expect(previousStep("library")).toBe("indexers");
    expect(previousStep("indexers")).toBe("provider");
    expect(previousStep("provider")).toBe("welcome");
    expect(previousStep("welcome")).toBeNull();
  });

  it("makes Escape one step back, and a no-op on Welcome", () => {
    // Escape is Back, not "dismiss": the wizard is the whole window, so there is no shell behind
    // it to dismiss to. A first-run user who pressed Escape would land on an empty application
    // with no route back to the wizard except a restart.
    const steps: readonly SetupStepId[] = ["welcome", "provider", "indexers", "library", "done"];
    for (const step of steps) {
      expect(escapeTarget(step)).toBe(previousStep(step));
    }
    expect(escapeTarget("welcome")).toBeNull();
  });
});

describe("leaving a step", () => {
  it("lets Welcome and Done continue without any fact", () => {
    expect(canContinue("welcome", NOTHING)).toBe(true);
    expect(canContinue("done", NOTHING)).toBe(true);
  });

  it("waits for each fact on the step that produces it", () => {
    expect(canContinue("provider", NOTHING)).toBe(false);
    expect(canContinue("provider", { ...NOTHING, providerReady: true })).toBe(true);
    expect(canContinue("indexers", NOTHING)).toBe(false);
    expect(canContinue("indexers", { ...NOTHING, indexersReady: true })).toBe(true);
    expect(canContinue("library", NOTHING)).toBe(false);
    expect(canContinue("library", { ...NOTHING, libraryReady: true })).toBe(true);
  });

  it("does not let one fact unlock another step", () => {
    const providerOnly: SetupFacts = {
      providerReady: true,
      indexersReady: false,
      libraryReady: false,
    };
    expect(canContinue("indexers", providerOnly)).toBe(false);
    expect(canContinue("library", providerOnly)).toBe(false);
    expect(canContinue("provider", READY)).toBe(true);
  });
});

describe("skipping", () => {
  it("offers a skip only on the steps whose loss is survivable", () => {
    // The library has no skip: an installation with no root has nothing to open, so the wizard
    // must not have a path that leaves the user with an empty application.
    expect(skipLabelKey("provider")).toBe("setup.provider.skip");
    expect(skipLabelKey("indexers")).toBe("setup.indexers.skip");
    expect(skipLabelKey("welcome")).toBeNull();
    expect(skipLabelKey("library")).toBeNull();
    expect(skipLabelKey("done")).toBeNull();
  });

  it("explains what each unavailable step is waiting for", () => {
    expect(continueHintKey("provider")).toBe("setup.provider.required");
    expect(continueHintKey("indexers")).toBe("setup.indexers.required");
    expect(continueHintKey("library")).toBe("setup.library.required");
    expect(continueHintKey("welcome")).toBeNull();
    expect(continueHintKey("done")).toBeNull();
  });
});
