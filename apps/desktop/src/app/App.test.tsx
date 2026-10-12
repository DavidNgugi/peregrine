import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { App } from "./App.js";

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
});
