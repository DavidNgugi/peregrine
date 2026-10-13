import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { Dialog } from "./Dialog.js";
import { createOverlayStack } from "./overlay-stack.js";

/**
 * The dialog as a static string.
 *
 * Effects do not run while a component is rendered to a string, which is exactly the part of a dialog
 * that can be asserted without a browser: the roles, the labelling, and the elevation. What cannot be
 * asserted here — focus moving in and out, and Escape arriving at the document — is stated in the
 * component and driven by `overlay-stack`, whose rule *is* tested. The alternative was a DOM
 * environment, and this repository's frontend tests run in Node on purpose.
 */
function markup(description?: string): string {
  return renderToStaticMarkup(
    <Dialog
      closeLabel="Close"
      // Spread rather than passing `undefined`: this repo compiles with
      // `exactOptionalPropertyTypes`, where an explicit `undefined` is not the same as an absent
      // prop — and the component's whole point here is that an absent description draws no
      // `aria-describedby`.
      {...(description === undefined ? {} : { description })}
      onClose={() => undefined}
      stack={createOverlayStack()}
      title="What is here"
    >
      <p>body</p>
    </Dialog>,
  );
}

describe("Dialog", () => {
  it("is a modal dialog labelled by its own heading", () => {
    const html = markup();

    expect(html).toContain('role="dialog"');
    expect(html).toContain('aria-modal="true"');
    expect(html).toContain("aria-labelledby=");
    expect(html).toContain("What is here");
  });

  it("associates its description only when there is one", () => {
    // `aria-describedby` pointing at an id nothing has is worse than no description: a screen reader
    // announces nothing and the markup claims otherwise.
    expect(markup("A sentence.")).toContain("aria-describedby=");
    expect(markup()).not.toContain("aria-describedby=");
  });

  it("floats on the design system's scrim and its dialog elevation", () => {
    // §4: elevation is a shadow *plus* a border, never a shadow alone, and a dialog is `--elev-3`.
    const html = markup();

    expect(html).toContain("bg-surface-scrim");
    expect(html).toContain("shadow-elev-3");
    expect(html).toContain("border-border-subtle");
  });

  it("names its close control rather than labelling it with the title", () => {
    const html = markup();

    expect(html).toContain('aria-label="Close"');
    expect(html).not.toContain('aria-label="What is here"');
  });

  it("draws its children inside itself", () => {
    const html = markup();

    expect(html).toContain("body");
    expect(html.indexOf("body")).toBeGreaterThan(html.indexOf('role="dialog"'));
  });
});
