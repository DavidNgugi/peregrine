import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { IconButton } from "./IconButton.js";

function classesOf(markup: string): string[] {
  const match = /class="([^"]*)"/u.exec(markup);
  return match?.[1]?.split(/\s+/u) ?? [];
}

describe("IconButton", () => {
  it("is named by its label and draws no text of its own", () => {
    const markup = renderToStaticMarkup(
      <IconButton icon="moon" label="Switch to the light theme" />,
    );
    expect(markup).toContain('aria-label="Switch to the light theme"');
    // Nothing between the tags: the icon is the only child, so the label is the name.
    expect(markup).toMatch(/<button[^>]*><svg/u);
    expect(markup).not.toContain("Switch to the light theme<");
  });

  it("defaults to type=button so it cannot submit a form by accident", () => {
    expect(renderToStaticMarkup(<IconButton icon="sun" label="Theme" />)).toContain(
      'type="button"',
    );
  });

  it("keeps an explicit type and a disabled state", () => {
    const markup = renderToStaticMarkup(
      <IconButton disabled icon="sun" label="Theme" type="submit" />,
    );
    expect(markup).toContain('type="submit"');
    expect(markup).toContain("disabled");
  });

  it("clears the pointer target minimum at both sizes (§4)", () => {
    expect(
      classesOf(renderToStaticMarkup(<IconButton icon="sun" label="Theme" size="sm" />)),
    ).toContain("size-7");
    expect(
      classesOf(renderToStaticMarkup(<IconButton icon="sun" label="Theme" size="md" />)),
    ).toContain("size-8");
    expect(
      classesOf(renderToStaticMarkup(<IconButton icon="sun" label="Theme" size="lg" />)),
    ).toContain("size-11");
  });

  it("scales the mark with the box rather than drawing a fixed one", () => {
    expect(renderToStaticMarkup(<IconButton icon="sun" label="Theme" size="sm" />)).toContain(
      'height="16"',
    );
    expect(renderToStaticMarkup(<IconButton icon="sun" label="Theme" />)).toContain('height="20"');
    expect(renderToStaticMarkup(<IconButton icon="sun" label="Theme" size="lg" />)).toContain(
      'height="24"',
    );
  });

  it("renders the token focus ring and the reduced-motion branch", () => {
    const classes = classesOf(renderToStaticMarkup(<IconButton icon="sun" label="Theme" />));
    expect(classes).toContain("focus-visible:outline-focus-ring");
    expect(classes).toContain("motion-reduce:transition-none");
  });

  it("lets the call site win a conflicting utility", () => {
    const classes = classesOf(
      renderToStaticMarkup(
        <IconButton className="bg-surface-2" icon="sun" label="Theme" variant="primary" />,
      ),
    );
    expect(classes).toContain("bg-surface-2");
    expect(classes).not.toContain("bg-accent");
  });
});
