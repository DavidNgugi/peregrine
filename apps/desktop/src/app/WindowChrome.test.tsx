import { createI18n } from "@peregrine/i18n";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it } from "vitest";
import type { Platform } from "../lib/platform.js";
import { WindowChrome } from "./WindowChrome.js";

const i18n = createI18n();

// The controls the shell puts in the bar, reduced to the one shape the drag region's
// hit-testing depends on: a real `<button>`, which is what Tauri's injected script
// classifies as clickable and refuses to drag.
const CONTROLS = (
  <nav>
    <button type="button">Discover</button>
  </nav>
);

function chromeMarkup(platform: Platform | null): string {
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <WindowChrome platform={platform}>{CONTROLS}</WindowChrome>
    </I18nextProvider>,
  );
}

function openingTag(markup: string, tag: string): string {
  return new RegExp(`<${tag}\\b[^>]*>`, "u").exec(markup)?.[0] ?? "";
}

function dragRegions(markup: string): string[] {
  return markup.match(/data-tauri-drag-region/gu) ?? [];
}

describe("WindowChrome", () => {
  it("makes the macOS bar a title bar: draggable, inset for the traffic lights, named", () => {
    const markup = chromeMarkup("macos");
    const header = openingTag(markup, "header");

    // The traffic lights are the OS's, drawn over this bar, so the bar reserves their
    // space and the app supplies the title the OS withheld.
    expect(header).toContain("pl-[78px]");
    expect(header).toContain("min-h-[52px]");
    expect(markup).toContain("Peregrine");
    expect(markup).toContain("Discover");
  });

  it("leaves the Windows bar a toolbar under the native frame", () => {
    const markup = chromeMarkup("windows");
    const header = openingTag(markup, "header");

    expect(header).toContain("min-h-[40px]");
    // The OS title bar carries the app name; repeating it here would read as two titles.
    expect(markup).not.toContain("Peregrine");
  });

  it("sizes the Linux bar like a header bar, with the desktop's own controls", () => {
    const header = openingTag(chromeMarkup("linux"), "header");

    expect(header).toContain("min-h-[47px]");
    expect(header).toContain("px-3");
    expect(header).not.toContain("pl-[78px]");
  });

  it("keeps the app name when there is no OS title bar to show it, and starts anyway", () => {
    const markup = chromeMarkup(null);

    // ARCHITECTURE §15: an unreadable platform degrades to the neutral bar rather than
    // stopping the shell, and no platform's geometry is assumed.
    expect(markup).toContain("Peregrine");
    expect(markup).toContain("Discover");
    expect(openingTag(markup, "header")).not.toContain("pl-[78px]");
  });

  it("declares one drag region, on the bar itself", () => {
    for (const platform of ["macos", "windows", "linux", null] as const) {
      const markup = chromeMarkup(platform);
      expect(dragRegions(markup), `${platform} has a stray drag region`).toHaveLength(1);
      expect(openingTag(markup, "header")).toContain("data-tauri-drag-region");
    }
  });

  it("drives the drag region only where the OS draws no title bar", () => {
    // "deep" lets the app name and the bar's own background drag the window; Tauri's
    // script still refuses to drag from any clickable element on the way up, and
    // "false" switches dragging off for the bar wherever the OS already owns it.
    expect(openingTag(chromeMarkup("macos"), "header")).toContain('data-tauri-drag-region="deep"');
    for (const platform of ["windows", "linux", null] as const) {
      expect(openingTag(chromeMarkup(platform), "header")).toContain(
        'data-tauri-drag-region="false"',
      );
    }
  });

  it("keeps the drag region out of the keyboard order", () => {
    // A focusable drag region is a focus trap that does nothing when activated.
    expect(chromeMarkup("macos")).not.toContain("tabindex");
  });

  it("draws no window control of its own on any platform", () => {
    // DESIGN_SYSTEM §8 forbids an invented caption button, and ARCHITECTURE §17 keeps the
    // native frame on Windows and Linux: the OS owns minimize, maximize, zoom, close and
    // the Snap Layouts that hang off the maximize button.
    for (const platform of ["macos", "windows", "linux", null] as const) {
      expect(chromeMarkup(platform)).not.toMatch(
        /aria-label="(Minimize|Maximize|Restore|Close|Zoom)\b/u,
      );
    }
  });
});

// The controls are the shell's; the chrome places them inside the bar, which is the only
// place the drag region's hit-testing applies to them.
describe("WindowChrome children", () => {
  it("places the app's controls inside the bar", () => {
    const markup = chromeMarkup("macos");
    const header = markup.indexOf("<header");
    const nav = markup.indexOf("<nav");
    const close = markup.indexOf("</header>");

    expect(header).toBeGreaterThanOrEqual(0);
    expect(nav).toBeGreaterThan(header);
    expect(nav).toBeLessThan(close);
  });
});
