import { createI18n } from "@peregrine/i18n";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { App } from "./App.js";
import { SCREENS } from "./screens.js";

const i18n = createI18n();

/** The sidebar's markup, so an assertion cannot accidentally match the screen. */
function sidebar(markup: string): string {
  const start = markup.indexOf("<nav");
  const end = markup.indexOf("</nav>");
  expect(start).toBeGreaterThan(-1);
  expect(end).toBeGreaterThan(start);
  return markup.slice(start, end);
}

/** The content area's markup — everything a screen is allowed to own. */
function contentArea(markup: string): string {
  const start = markup.indexOf("<main");
  const end = markup.indexOf("</main>");
  expect(start).toBeGreaterThan(-1);
  expect(end).toBeGreaterThan(start);
  return markup.slice(start, end);
}

describe("App", () => {
  it("composes the shell, the current screen, and the player bar", () => {
    const markup = renderToStaticMarkup(<App platform="macos" />);
    expect(markup).toContain("Peregrine");
    expect(markup).toContain("Discover");
    expect(markup).toContain("Nothing here yet");
    expect(markup).toContain("Nothing playing");
  });

  it("offers a reachable theme control with an accessible name", () => {
    const markup = renderToStaticMarkup(<App platform="macos" />);
    expect(markup).toContain("Switch to the light theme");
    expect(markup).toContain("<button");
  });

  it("presents the platform's chrome around the app's own controls", () => {
    expect(renderToStaticMarkup(<App platform="macos" />)).toContain(
      'data-tauri-drag-region="deep"',
    );
    expect(renderToStaticMarkup(<App platform="windows" />)).toContain(
      'data-tauri-drag-region="false"',
    );
  });

  it("puts the chrome, the navigation, and the player bar outside the screen's own area", () => {
    // The shell's persistence is structural: PG-E7-01's acceptance is that navigation
    // never unmounts the shell, and nothing a screen renders can take the chrome or the
    // player with it while they are siblings of `main` rather than inside it.
    const markup = renderToStaticMarkup(<App platform="macos" />);
    const content = contentArea(markup);

    expect(content).toContain("Nothing here yet");
    expect(content).not.toContain("data-tauri-drag-region");
    expect(content).not.toContain("Nothing playing");
    expect(content).not.toContain('aria-label="Screens"');
  });

  it("lists one sidebar row per screen, in the order the shell declares them", () => {
    const nav = sidebar(renderToStaticMarkup(<App platform="macos" />));
    expect(nav.match(/<button/gu)).toHaveLength(SCREENS.length);

    // The labels come from the catalogue rather than from a copy of it here, so a screen
    // added with a new key is covered by this test rather than silently untested.
    let previous = -1;
    for (const entry of SCREENS) {
      const at = nav.indexOf(i18n.t(entry.labelKey));
      expect(at, entry.labelKey).toBeGreaterThan(previous);
      previous = at;
    }
  });

  it("marks only the screen on show as the current row", () => {
    const nav = sidebar(renderToStaticMarkup(<App platform="macos" />));
    expect(nav.match(/aria-current="page"/gu)).toHaveLength(1);
    expect(nav.indexOf("aria-current")).toBeLessThan(nav.indexOf("Diagnostics"));
  });
});
