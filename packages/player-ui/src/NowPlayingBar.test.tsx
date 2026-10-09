import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { NowPlayingBar } from "./NowPlayingBar.js";

describe("NowPlayingBar", () => {
  it("shows the idle label and no transport", () => {
    const markup = renderToStaticMarkup(
      <NowPlayingBar state={{ kind: "idle" }} idleLabel="Nothing playing" />,
    );
    expect(markup).toContain("Nothing playing");
    expect(markup).not.toContain("progressbar");
  });

  it("shows the title, both timecodes, and the progress value", () => {
    const markup = renderToStaticMarkup(
      <NowPlayingBar
        idleLabel="Nothing playing"
        state={{ kind: "playing", title: "Arrival", positionMs: 30_000, durationMs: 120_000 }}
      />,
    );
    expect(markup).toContain("Arrival");
    expect(markup).toContain("0:30 / 2:00");
    expect(markup).toContain('aria-valuenow="25"');
    expect(markup).toContain("scaleX(0.25)");
  });

  it("announces state changes politely rather than assertively", () => {
    const markup = renderToStaticMarkup(
      <NowPlayingBar state={{ kind: "idle" }} idleLabel="Nothing playing" />,
    );
    expect(markup).toContain('role="status"');
    expect(markup).toContain('aria-live="polite"');
  });
});
