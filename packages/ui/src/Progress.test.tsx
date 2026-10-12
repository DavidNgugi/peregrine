import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { Progress } from "./Progress.js";

function fill(markup: string): string {
  return /style="([^"]*)"/u.exec(markup)?.[1] ?? "";
}

describe("Progress", () => {
  it("announces the figure rather than only drawing it", () => {
    const markup = renderToStaticMarkup(<Progress label="Job progress" value={42} />);
    expect(markup).toContain('role="progressbar"');
    expect(markup).toContain('aria-label="Job progress"');
    expect(markup).toContain('aria-valuemin="0"');
    expect(markup).toContain('aria-valuemax="100"');
    expect(markup).toContain('aria-valuenow="42"');
  });

  it("moves the fill with a transform and never with a layout property", () => {
    // §5 rule 2 allows transform and opacity only, and AGENTS §6 forbids an effect that
    // needs layout during interaction. A width animation lays the bar out every frame.
    const markup = renderToStaticMarkup(<Progress label="Job progress" value={25} />);
    expect(fill(markup)).toBe("transform:scaleX(0.25)");
    expect(markup).not.toContain("transition-[width]");
    expect(markup).not.toContain("width:");
    expect(markup).toContain("origin-left");
  });

  it("clamps a figure that arrives out of range", () => {
    // The number comes from the core over IPC and from a journal row; a bar asked for 120%
    // would paint outside its track.
    expect(fill(renderToStaticMarkup(<Progress label="p" value={120} />))).toBe(
      "transform:scaleX(1)",
    );
    expect(fill(renderToStaticMarkup(<Progress label="p" value={-5} />))).toBe(
      "transform:scaleX(0)",
    );
  });

  it("draws an empty bar rather than a broken one for a figure that is not a number", () => {
    const markup = renderToStaticMarkup(<Progress label="p" value={Number.NaN} />);
    expect(fill(markup)).toBe("transform:scaleX(0)");
    expect(markup).toContain('aria-valuenow="0"');
  });

  it("renders the reduced-motion branch", () => {
    expect(renderToStaticMarkup(<Progress label="p" value={1} />)).toContain(
      "motion-reduce:transition-none",
    );
  });
});
