import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { App } from "./App.js";

describe("App", () => {
  it("composes the shell, the current screen, and the player bar", () => {
    const markup = renderToStaticMarkup(<App />);
    expect(markup).toContain("Peregrine");
    expect(markup).toContain("Discover");
    expect(markup).toContain("Nothing here yet");
    expect(markup).toContain("Nothing playing");
  });

  it("offers a reachable theme control with an accessible name", () => {
    const markup = renderToStaticMarkup(<App />);
    expect(markup).toContain("Switch to the light theme");
    expect(markup).toContain("<button");
  });
});
