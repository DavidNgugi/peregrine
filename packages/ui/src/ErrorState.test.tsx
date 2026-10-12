import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { ErrorState } from "./ErrorState.js";

describe("ErrorState", () => {
  it("announces itself when it appears", () => {
    // The half of an error a visual treatment loses: a screen reader has to be told
    // without the user going looking for it (PG-E21-19).
    expect(renderToStaticMarkup(<ErrorState title="Could not save." />)).toContain('role="alert"');
  });

  it("says what failed and, when it can, why", () => {
    const markup = renderToStaticMarkup(
      <ErrorState body="Your account may not allow it." title="Could not save." />,
    );
    expect(markup).toContain("Could not save.");
    expect(markup).toContain("Your account may not allow it.");
  });

  it("draws the tone as an indicator rather than as body text", () => {
    // `--state-danger` is only held to 3:1 — the control and indicator threshold — while
    // body text is held to 4.5:1, so the colour belongs on the border and not on the copy
    // (docs/DESIGN_SYSTEM.md §2.2).
    const markup = renderToStaticMarkup(<ErrorState title="Could not save." />);
    expect(markup).toContain("border-state-danger");
    expect(markup).toContain("text-text-primary");
    expect(markup).not.toContain("text-state-danger");
  });

  it("draws one line when there is no more to say", () => {
    const markup = renderToStaticMarkup(<ErrorState title="Could not save." />);
    expect(markup.match(/<p/gu)).toHaveLength(1);
  });

  it("offers an action without making it part of the alert's content", () => {
    // The control lives outside the text, so a screen reader announcing the alert reads
    // what happened and not "Dismiss" in the middle of it.
    const markup = renderToStaticMarkup(
      <ErrorState
        action={<button type="button">Dismiss</button>}
        body="It came back with a code."
        title="Could not save."
      />,
    );
    expect(markup).toContain("Dismiss");
    expect(markup.indexOf("</p>")).toBeLessThan(markup.indexOf("<button"));
  });

  it("draws no action when the caller has none", () => {
    expect(renderToStaticMarkup(<ErrorState title="Could not save." />)).not.toContain("<button");
  });
});
