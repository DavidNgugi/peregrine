import { createI18n } from "@peregrine/i18n";
import type { CredentialStoreDto, LibraryRootDto, ProwlarrStatusDto } from "@peregrine/ipc-client";
import type { ReactElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it } from "vitest";
import type { SetupFacts, SetupStepId } from "../setup-steps.js";
import { DoneStep } from "./DoneStep.js";
import { IndexersStep } from "./IndexersStep.js";
import { LibraryStep, type SetupScanState } from "./LibraryStep.js";
import { ProviderStep, type SetupProviderView } from "./ProviderStep.js";
import { SetupFooter } from "./SetupFooter.js";
import { SetupProgress } from "./SetupProgress.js";
import { SetupWizard } from "./SetupWizard.js";

const i18n = createI18n();

function render(ui: ReactElement): string {
  return renderToStaticMarkup(<I18nextProvider i18n={i18n}>{ui}</I18nextProvider>);
}

/**
 * Whether the button drawing `label` is disabled, read from the markup.
 *
 * The label can appear more than once — as a heading, as a sentence, and as a button — so this
 * reads the last button that draws it. `disabled=""` is matched rather than the word, because
 * every button's class list carries Tailwind's `disabled:` variants.
 */
function disabledButton(html: string, label: string): boolean {
  const end = html.lastIndexOf(`>${label}</button>`);
  if (end === -1) {
    return false;
  }
  return html.slice(html.lastIndexOf("<button", end), end).includes('disabled=""');
}

function prowlarr(overrides: Partial<ProwlarrStatusDto> = {}): ProwlarrStatusDto {
  return {
    supported: true,
    installed: false,
    running: false,
    version: "1.0.0",
    baseUrl: null,
    phase: "idle",
    progressPermille: 0,
    ...overrides,
  };
}

function root(overrides: Partial<LibraryRootDto> = {}): LibraryRootDto {
  return {
    id: 1,
    path: "/media/films",
    kind: "mixed",
    scanMode: "incremental",
    lastScanAt: null,
    createdAt: 0,
    ...overrides,
  };
}

const NOTHING: SetupFacts = {
  providerReady: false,
  indexersReady: false,
  libraryReady: false,
};

const READY: SetupFacts = {
  providerReady: true,
  indexersReady: true,
  libraryReady: true,
};

const VIEW: SetupProviderView = {
  provider: "tmdb",
  displayName: "TMDB",
  configured: false,
  builtIn: false,
};

interface ProviderOverrides {
  readonly view?: SetupProviderView | null;
  readonly store?: CredentialStoreDto;
  readonly loading?: boolean;
  readonly failed?: boolean;
  readonly busy?: boolean;
  readonly outcome?: { readonly key: string; readonly ok: boolean } | null;
  readonly error?: string | null;
  readonly onUseBuiltIn?: (() => Promise<void>) | undefined;
}

function providerStep(overrides: ProviderOverrides = {}): string {
  return render(
    <ProviderStep
      busy={false}
      error={null}
      failed={false}
      loading={false}
      onRetryStore={() => undefined}
      onSave={() => Promise.resolve()}
      onTest={() => Promise.resolve()}
      outcome={null}
      store="answered"
      view={VIEW}
      {...overrides}
    />,
  );
}

interface IndexerOverrides {
  readonly status?: ProwlarrStatusDto | null;
  readonly loading?: boolean;
  readonly failed?: boolean;
  readonly busy?: boolean;
  readonly phase?: string;
  readonly progressPermille?: number;
  readonly error?: string | null;
}

function indexerStep(overrides: IndexerOverrides = {}): string {
  return render(
    <IndexersStep
      busy={false}
      error={null}
      failed={false}
      loading={false}
      onInstall={() => Promise.resolve()}
      onStart={() => Promise.resolve()}
      phase="idle"
      progressPermille={0}
      status={prowlarr()}
      {...overrides}
    />,
  );
}

interface LibraryOverrides {
  readonly roots?: readonly LibraryRootDto[];
  readonly loading?: boolean;
  readonly failed?: boolean;
  readonly adding?: boolean;
  readonly busy?: boolean;
  readonly error?: string | null;
  readonly scan?: SetupScanState;
}

function libraryStep(overrides: LibraryOverrides = {}): string {
  return render(
    <LibraryStep
      adding={false}
      busy={false}
      error={null}
      failed={false}
      loading={false}
      onChoose={() => Promise.resolve()}
      onScan={() => Promise.resolve()}
      roots={[]}
      scan="idle"
      {...overrides}
    />,
  );
}

function footer(step: SetupStepId, facts: SetupFacts): string {
  return render(
    <SetupFooter
      facts={facts}
      onBack={() => undefined}
      onNext={() => undefined}
      onSkip={() => undefined}
      step={step}
    />,
  );
}

describe("SetupWizard", () => {
  it("opens on Welcome with a fixed frame, a progress indicator, and no sidebar", () => {
    const html = render(<SetupWizard onFinished={() => undefined} />);

    expect(html).toContain("Welcome to Peregrine");
    expect(html).toContain("Step 1 of 5");
    expect(html).toContain('data-setup-frame=""');
    // The wizard is the whole window: the content area clips rather than becoming a second
    // scroller, and the shell's navigation is not rendered beside it.
    expect(html).not.toContain("overflow-y-auto");
    expect(html).not.toContain('aria-label="Screens"');
    expect(html.match(/aria-current="step"/gu)).toHaveLength(1);
    expect(disabledButton(html, "Back")).toBe(true);
    expect(html).toContain("Get started");
    expect(html).not.toContain("Skip for now");
  });
});

describe("SetupProgress", () => {
  it("marks one current step and names every step for assistive technology", () => {
    const html = render(<SetupProgress current="provider" />);

    expect(html).toContain("Step 2 of 5");
    expect(html.match(/aria-current="step"/gu)).toHaveLength(1);
    for (const label of ["Welcome", "Provider", "Indexers", "Library", "Done"]) {
      expect(html).toContain(`>${label}<`);
    }
  });
});

describe("ProviderStep", () => {
  it("paints its heading before the keychain has answered", () => {
    const html = providerStep({ loading: true, view: null });

    expect(html).toContain("<h1");
    expect(html).toContain("Checking your keychain…");
  });

  it("asks for a key with a password field and the save action", () => {
    const html = providerStep();

    expect(html).toContain('type="password"');
    expect(html).toContain("Save and test");
    // Only a stored key can be tested, so the test control appears once one exists.
    expect(html).not.toContain("Test connection");
  });

  it("offers the connection test once a key is stored", () => {
    const html = providerStep({ view: { ...VIEW, configured: true } });

    expect(html).toContain("TMDB is connected.");
    expect(html).toContain("Test connection");
  });

  it("becomes one click when a built-in key exists", () => {
    const html = providerStep({
      view: { ...VIEW, builtIn: true },
      onUseBuiltIn: () => Promise.resolve(),
    });

    expect(html).toContain("Use the built-in key");
    expect(html).not.toContain('type="password"');
  });

  it("says the keychain refused rather than that no key is stored", () => {
    const html = providerStep({ store: "denied" });

    expect(html).toContain("cannot read your keychain");
    expect(html).toContain("Try again");
  });

  it("reports a refused key with the sentence for the code the job recorded", () => {
    const html = providerStep({
      outcome: { key: "settings.providers.outcome.unauthorized", ok: false },
    });

    expect(html).toContain(
      "That key was refused. Check you copied the API key and not another value.",
    );
    expect(html).toContain('role="alert"');
  });

  it("reports a stored key as news rather than as a failure", () => {
    const html = providerStep({
      view: { ...VIEW, configured: true },
      outcome: { key: "settings.providers.outcome.saved", ok: true },
    });

    expect(html).toContain("That key works. Peregrine stored it for TMDB.");
    expect(html).toContain('role="status"');
  });

  it("distinguishes a build with no provider from a failed read", () => {
    const empty = providerStep({ view: null });
    const failed = providerStep({ failed: true });

    expect(empty).toContain("This build has no metadata provider to configure.");
    expect(failed).toContain("Your provider could not be read.");
    expect(failed).not.toContain("This build has no metadata provider to configure.");
  });
});

describe("IndexersStep", () => {
  it("offers the one-click install when Prowlarr is not installed", () => {
    const html = indexerStep();

    expect(html).toContain("Set up indexers");
    expect(html).toContain("Install Prowlarr");
  });

  it("shows install progress in place rather than a full-page spinner", () => {
    const html = indexerStep({
      busy: true,
      phase: "installing",
      progressPermille: 420,
      status: prowlarr({ phase: "installing", progressPermille: 420 }),
    });

    expect(html).toContain('role="progressbar"');
    expect(html).toContain('aria-valuenow="42"');
    expect(html).toContain("Installing Prowlarr…");
    expect(disabledButton(html, "Installing Prowlarr…")).toBe(true);
  });

  it("says search works once the instance is running", () => {
    const html = indexerStep({ status: prowlarr({ installed: true, running: true }) });

    expect(html).toContain("Prowlarr is running. Search works.");
    expect(html).not.toContain(">Install Prowlarr<");
  });

  it("says this platform has no managed build instead of offering one", () => {
    const html = indexerStep({ status: prowlarr({ supported: false }) });

    expect(html).toContain("no managed Prowlarr build");
    expect(html).toContain("search will be unavailable");
    expect(html).not.toContain(">Install Prowlarr<");
  });

  it("distinguishes a failed read from an unsupported platform", () => {
    const html = indexerStep({ failed: true });

    expect(html).toContain("The managed Prowlarr could not be read.");
    expect(html).not.toContain("no managed Prowlarr build");
  });
});

describe("LibraryStep", () => {
  it("asks for a folder before anything has been chosen", () => {
    const html = libraryStep();

    expect(html).toContain("Choose your library");
    expect(html).toContain("Choose folder…");
    expect(html).toContain("No folder chosen yet.");
  });

  it("names the folder once it is a root and offers another", () => {
    const html = libraryStep({ roots: [root()] });

    expect(html).toContain("/media/films");
    expect(html).toContain("Add another folder…");
    expect(html).not.toContain("Choose folder…");
  });

  it("says the scan started after the root is added", () => {
    const html = libraryStep({ roots: [root()], scan: "started" });

    expect(html).toContain("Scan started. Titles appear as the scan finds them.");
  });

  it("keeps the root and offers a retry when the scan could not start", () => {
    const html = libraryStep({
      roots: [root()],
      scan: "failed",
      error: "A scan is already running.",
    });

    expect(html).toContain("The folder was added, but the scan could not start.");
    expect(html).toContain("A scan is already running.");
    expect(html).toContain("Scan now");
    expect(html).toContain("/media/films");
  });

  it("shows a refused folder in the step rather than as a missing root", () => {
    const html = libraryStep({ error: "That folder cannot be read." });

    expect(html).toContain("That folder could not be added.");
    expect(html).toContain("That folder cannot be read.");
  });

  it("still offers the folder control when the read failed", () => {
    // The read failing is not the same as the core being unable to accept a folder; refusing the
    // click would leave the wizard with no way forward and no skip.
    const html = libraryStep({ failed: true });

    expect(html).toContain("Your folders could not be read.");
    expect(disabledButton(html, "Choose folder…")).toBe(false);
  });
});

describe("DoneStep", () => {
  it("summarises what was skipped as well as what was configured", () => {
    const html = render(
      <DoneStep indexers="skipped" libraryCount={0} providerConfigured={false} />,
    );

    expect(html).toContain("Metadata provider: not configured.");
    expect(html).toContain("Search: not set up.");
    expect(html).toContain("Library: 0 folders.");
  });

  it("summarises a complete setup with the library count", () => {
    const html = render(<DoneStep indexers="ready" libraryCount={2} providerConfigured />);

    expect(html).toContain("Metadata provider: connected.");
    expect(html).toContain("Search: Prowlarr is ready.");
    expect(html).toContain("Library: 2 folders.");
    expect(html).not.toContain("not configured");
  });

  it("does not claim search works on a platform without a managed build", () => {
    // The step lets the user through, but the summary must not turn "there is no build for this
    // platform" into "search is ready".
    const html = render(<DoneStep indexers="unsupported" libraryCount={1} providerConfigured />);

    expect(html).toContain("this platform has no managed Prowlarr");
    expect(html).not.toContain("Search: Prowlarr is ready.");
  });
});

describe("SetupFooter", () => {
  it("starts with a disabled Back and the welcome action", () => {
    const html = footer("welcome", NOTHING);

    expect(disabledButton(html, "Back")).toBe(true);
    expect(html).toContain("Get started");
    expect(html).not.toContain("Skip for now");
  });

  it("offers a labelled skip on the provider step and says what it costs", () => {
    const html = footer("provider", NOTHING);

    expect(html).toContain("Skip for now");
    expect(html).toContain("Discover and Browse will stay empty.");
    expect(html).toContain("Save a key to continue.");
    expect(disabledButton(html, "Continue")).toBe(true);
  });

  it("removes the provider skip once a key is configured", () => {
    const html = footer("provider", READY);

    expect(html).not.toContain("Skip for now");
    expect(html).not.toContain("Save a key to continue.");
    expect(disabledButton(html, "Continue")).toBe(false);
  });

  it("offers a skip that says search will be unavailable", () => {
    const html = footer("indexers", NOTHING);

    expect(html).toContain("Skip — search will be unavailable.");
    expect(html).toContain("Install Prowlarr to continue.");
  });

  it("never offers to skip the library", () => {
    const html = footer("library", NOTHING);

    expect(html).not.toContain("Skip");
    expect(html).toContain("Add a folder to continue.");
    expect(disabledButton(html, "Continue")).toBe(true);
  });

  it("leaves the wizard from Done with one action", () => {
    const html = footer("done", READY);

    expect(html).toContain("Open Peregrine");
    expect(html).not.toContain("Skip");
  });
});
