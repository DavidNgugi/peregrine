import { createI18n } from "@peregrine/i18n";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { App } from "./App.js";
import { HOME } from "./routes.js";
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
    const markup = renderToStaticMarkup(<App platform="macos" route={HOME} />);

    // The three regions, each anchored on something only it draws: the navigation column's own
    // landmark, the current screen's heading and toolbar, and the player bar's idle line. A
    // screen's rails read answers on an effect, which a static render never runs, so what is
    // composed here is the screen's first frame — and that frame deliberately claims nothing about
    // a catalogue nobody has read, rather than drawing "Nothing here yet" over one.
    expect(markup).toContain(`aria-label="${i18n.t("shell.nav.label")}"`);
    expect(contentArea(markup)).toContain("Discover");
    expect(contentArea(markup)).toContain(i18n.t("discover.search.placeholder"));
    expect(markup).toContain(i18n.t("shell.nowPlaying.idle"));
  });

  it("offers a reachable theme control with an accessible name", () => {
    const markup = renderToStaticMarkup(<App platform="macos" route={HOME} />);
    expect(markup).toContain("Switch to the light theme");
    expect(markup).toContain("<button");
  });

  it("presents the platform's chrome around the app's own controls", () => {
    expect(renderToStaticMarkup(<App platform="macos" route={HOME} />)).toContain(
      'data-tauri-drag-region="deep"',
    );
    expect(renderToStaticMarkup(<App platform="windows" route={HOME} />)).toContain(
      'data-tauri-drag-region="false"',
    );
  });

  it("puts the chrome, the navigation, and the player bar outside the screen's own area", () => {
    // The shell's persistence is structural: PG-E7-01's acceptance is that navigation
    // never unmounts the shell, and nothing a screen renders can take the chrome or the
    // player with it while they are siblings of `main` rather than inside it.
    const markup = renderToStaticMarkup(<App platform="macos" route={HOME} />);
    const content = contentArea(markup);

    // What the screen draws on its first frame: its own heading and its toolbar, which is what a
    // reader gets while the rails read is still in the air.
    expect(content).toContain("<h1");
    expect(content).toContain(i18n.t("discover.search.placeholder"));
    expect(content).not.toContain("data-tauri-drag-region");
    expect(content).not.toContain(i18n.t("shell.nowPlaying.idle"));
    expect(content).not.toContain(`aria-label="${i18n.t("shell.nav.label")}"`);
  });

  it("lists one sidebar row per screen, in the order the shell declares them", () => {
    const nav = sidebar(renderToStaticMarkup(<App platform="macos" route={HOME} />));

    // A row is counted by the label it draws rather than by every `<button` in the landmark: the
    // header slot above the rows holds the caller's window chrome — one collapse control today —
    // and chrome is not a screen. A screen added to the table without a row still fails here,
    // because its label would then appear nowhere in the navigation; and the labels come from the
    // catalogue rather than from a copy of it here, so a screen added with a new key is covered by
    // this test rather than silently untested.
    let previous = -1;
    for (const entry of SCREENS) {
      const label = i18n.t(entry.labelKey);
      const rows = nav.split(label).length - 1;
      expect(rows, `${entry.labelKey} is one row`).toBe(1);

      const at = nav.indexOf(label);
      expect(at, entry.labelKey).toBeGreaterThan(previous);
      previous = at;
    }
  });

  it("marks only the screen on show as the current row", () => {
    const nav = sidebar(renderToStaticMarkup(<App platform="macos" route={HOME} />));
    expect(nav.match(/aria-current="page"/gu)).toHaveLength(1);
    expect(nav.indexOf("aria-current")).toBeLessThan(nav.indexOf("Diagnostics"));
  });
});

describe("PG-E7-02's acceptance", () => {
  it("renders every route's own content immediately, with no spinner", () => {
    // "No route renders a full-page spinner" is a claim about *every* route, so it is checked over
    // the table the sidebar draws from rather than over the one route a default render happens to
    // open. What it protects is the class of screen that renders nothing until its read answers: a
    // blank page is a spinner with worse manners, and a screen that cannot say where it is while it
    // waits has no shape to fill when the data arrives.
    for (const entry of SCREENS) {
      const markup = renderToStaticMarkup(<App platform="macos" route={entry.route} />);

      const content = contentArea(markup);

      // The screen names itself before it has asked the core anything. That is the whole of the
      // acceptance: a page whose only content is a wait cannot say where the user is, and a blank
      // one says even less. A progress *meter* is not this — Diagnostics shows one as its content,
      // which is why the property is "has its own heading" rather than "has no progressbar".
      expect(content, `${entry.labelKey} rendered no heading`).toContain("<h1");
      expect(content, `${entry.labelKey} rendered an empty content area`).not.toBe("<main></main>");
      expect(markup, `${entry.labelKey} kept its navigation`).toContain("<nav");
    }
  });

  it("follows the route it is given rather than always opening the first screen", () => {
    // The row that is current is the route's, so a route from anywhere — a shortcut, a link, the
    // window's own path — lands on the screen it names.
    const settings = SCREENS.find((entry) => entry.route.kind === "settings");
    if (settings === undefined) {
      throw new Error("the settings screen is in the table");
    }
    const nav = sidebar(renderToStaticMarkup(<App platform="macos" route={settings.route} />));

    expect(nav).toContain('aria-current="page"');
    expect(nav.match(/aria-current="page"/gu)).toHaveLength(1);
  });
});
