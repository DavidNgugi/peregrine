import { createI18n } from "@peregrine/i18n";
import type { ShortcutBinding } from "@peregrine/ipc-client";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it } from "vitest";
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

/**
 * The screen as markup rather than as a mounted tree.
 *
 * The list is a projection of the bindings and nothing else, so static rendering is
 * enough to see what it drew; the login-item hook's effect does not run, which is what
 * keeps this test off the IPC bridge.
 */
function markup(bindings: readonly ShortcutBinding[]): string {
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <SettingsScreen bindings={bindings} />
    </I18nextProvider>,
  );
}

describe("SettingsScreen", () => {
  it("draws one row per registry entry, in the registry's order", () => {
    const html = markup([
      binding("navigation.discover", "shortcuts.discover", "⌘1", "Digit1"),
      binding("navigation.jobs", "shortcuts.jobs", "⌘2", "Digit2"),
    ]);

    expect(html).toContain("Go to Discover");
    expect(html).toContain("Go to Diagnostics");
    expect(html.indexOf("Go to Discover")).toBeLessThan(html.indexOf("Go to Diagnostics"));
  });

  it("shows the accelerator the core rendered rather than computing one", () => {
    const html = markup([binding("navigation.settings", "shortcuts.settings", "⌥⇧⌘K", "KeyK")]);

    expect(html).toContain("⌥⇧⌘K");
    expect(html).toContain("<kbd");
  });

  it("draws no rows when the registry is empty, and still names the section", () => {
    const html = markup([]);

    expect(html).toContain("Keyboard shortcuts");
    expect(html).not.toContain("<kbd");
  });
});
