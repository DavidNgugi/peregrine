import { createI18n } from "@peregrine/i18n";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it } from "vitest";
import type { FirstRunCheck } from "../features/setup/index.js";
import { HOME } from "./routes.js";
import { SetupGateView } from "./SetupGate.js";

const i18n = createI18n();

function gate(check: FirstRunCheck): string {
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <SetupGateView check={check} platform="macos" route={HOME} />
    </I18nextProvider>,
  );
}

describe("SetupGate", () => {
  it("draws the wizard instead of the sidebar on a first run", () => {
    const html = gate({ state: "first-run" });

    expect(html).toContain("Welcome to Peregrine");
    expect(html).toContain('data-setup-frame=""');
    // The shell's navigation is the thing the wizard replaces, not a sibling it sits beside.
    expect(html).not.toContain(`aria-label="${i18n.t("shell.nav.label")}"`);
    expect(html).not.toContain("Discover");
  });

  it("keeps the application while the check is still in the air", () => {
    // A check that has not answered is not evidence of a first run. Showing the wizard here would
    // flash setup at every returner for the duration of two local reads.
    const html = gate({ state: "checking" });

    expect(html).toContain(`aria-label="${i18n.t("shell.nav.label")}"`);
    expect(html).not.toContain("Welcome to Peregrine");
  });

  it("keeps the application when the check could not answer", () => {
    // A failed read is even less evidence of a first run than an unfinished one; the user can
    // repair the library from Settings, and "set up again" would be a false instruction.
    const html = gate({ state: "unknown" });

    expect(html).toContain(`aria-label="${i18n.t("shell.nav.label")}"`);
    expect(html).not.toContain("Welcome to Peregrine");
  });

  it("draws the application once the installation has been set up", () => {
    const html = gate({ state: "configured" });

    expect(html).toContain(`aria-label="${i18n.t("shell.nav.label")}"`);
    expect(html).toContain("Discover");
    expect(html).not.toContain("Welcome to Peregrine");
  });
});
