import { createI18n } from "@peregrine/i18n";
import type { ShortcutBinding } from "@peregrine/ipc-client";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it } from "vitest";
import type { LibraryRoots } from "../hooks/useLibraryRoots.js";
import type { ProviderSettings } from "../hooks/useProviders.js";
import type { ShortcutRegistry } from "../hooks/useShortcuts.js";
import { SettingsScreen } from "./SettingsScreen.js";

const i18n = createI18n();

/** One row as the core sends it, with only the fields the list draws. */
function binding(id: string, labelKey: string, display: string, code: string): ShortcutBinding {
  return {
    id,
    labelKey,
    display,
    code,
    meta: true,
    ctrl: false,
    alt: false,
    shift: false,
  };
}

function registry(bindings: readonly ShortcutBinding[]): ShortcutRegistry {
  return { bindings, loading: false, failed: false };
}

/** The library read as a resolved, empty one: this suite is about the shortcut list. */
function noLibraries(): LibraryRoots {
  return {
    roots: [],
    loading: false,
    failed: false,
    busy: false,
    error: null,
    add: () => Promise.resolve(null),
    remove: () => Promise.resolve(false),
    scan: () => Promise.resolve(null),
  };
}

/** The provider read as a resolved, empty one: this suite is about the shortcut list. */
function noProviders(): ProviderSettings {
  return {
    providers: [],
    loading: false,
    failed: false,
    busy: false,
    outcome: null,
    error: null,
    saveKey: () => Promise.resolve(),
    testConnection: () => Promise.resolve(),
    removeKey: () => Promise.resolve(),
  };
}

/**
 * The screen as markup rather than as a mounted tree.
 *
 * The shortcut list is a projection of the registry and nothing else, so static rendering
 * shows what it drew; the login-item hook's effect does not run, which is what keeps this
 * test off the IPC bridge.
 */
function markup(
  shortcuts: ShortcutRegistry,
  libraries: LibraryRoots = noLibraries(),
  providers: ProviderSettings = noProviders(),
): string {
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <SettingsScreen libraries={libraries} providers={providers} shortcuts={shortcuts} />
    </I18nextProvider>,
  );
}

describe("SettingsScreen", () => {
  it("draws one row per registry entry, in the registry's order", () => {
    const html = markup(
      registry([
        binding("navigation.discover", "shortcuts.discover", "⌘1", "Digit1"),
        binding("navigation.jobs", "shortcuts.jobs", "⌘2", "Digit2"),
      ]),
    );

    expect(html).toContain("Go to Discover");
    expect(html).toContain("Go to Diagnostics");
    expect(html.indexOf("Go to Discover")).toBeLessThan(html.indexOf("Go to Diagnostics"));
  });

  it("shows the accelerator the core rendered rather than computing one", () => {
    const html = markup(
      registry([binding("navigation.settings", "shortcuts.settings", "⌥⇧⌘K", "KeyK")]),
    );

    expect(html).toContain("⌥⇧⌘K");
    expect(html).toContain("<kbd");
  });

  it("says the list is empty when the registry answered and had nothing in it", () => {
    const html = markup(registry([]));

    expect(html).toContain("Keyboard shortcuts");
    expect(html).toContain("No shortcut is registered in this build.");
    expect(html).not.toContain("<kbd");
  });

  it("says the list is unknown when the read failed, and never that it is empty", () => {
    // An unreachable core answers nothing. Telling the user they have no shortcuts would
    // be telling them something the app does not know.
    const html = markup({ bindings: [], failed: true, loading: false });

    expect(html).toContain("The shortcut list could not be read.");
    expect(html).not.toContain("No shortcut is registered in this build.");
  });

  it("draws neither an empty nor a failed list while the read is still in flight", () => {
    const html = markup({ bindings: [], failed: false, loading: true });

    expect(html).toContain("Keyboard shortcuts");
    expect(html).not.toContain("No shortcut is registered in this build.");
    expect(html).not.toContain("The shortcut list could not be read.");
  });

  it("reports a failure through an alert rather than through colour", () => {
    const html = markup({ bindings: [], failed: true, loading: false });

    expect(html).toContain('role="alert"');
  });

  it("draws the toggle as a switch that names its state", () => {
    const html = markup(registry([]));

    expect(html).toContain('role="switch"');
    expect(html).toContain("Start Peregrine when you sign in");
  });
});
