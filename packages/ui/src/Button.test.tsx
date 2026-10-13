import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { Button } from "./Button.js";

function classesOf(markup: string): string[] {
  const match = /class="([^"]*)"/u.exec(markup);
  return match?.[1]?.split(/\s+/u) ?? [];
}

describe("Button", () => {
  it("defaults to type=button so it cannot submit a form by accident", () => {
    expect(renderToStaticMarkup(<Button>Play</Button>)).toContain('type="button"');
  });

  it("keeps an explicit type and disabled state", () => {
    const markup = renderToStaticMarkup(
      <Button type="submit" disabled>
        Save
      </Button>,
    );
    expect(markup).toContain('type="submit"');
    expect(markup).toContain("disabled");
  });

  it("never breaks a two-word label across lines", () => {
    // Found by looking at the settings screen rather than by a test: "Scan now" sat in a row whose
    // description was wide, and the label drew as "Scan" over "now" — which reads as a rendering
    // fault, not as a wrapped label. The class is the contract; this is what keeps it.
    const markup = renderToStaticMarkup(<Button size="sm">Scan now</Button>);

    expect(classesOf(markup)).toContain("whitespace-nowrap");
  });

  it("renders the requested variant and size", () => {
    const classes = classesOf(
      renderToStaticMarkup(
        <Button variant="primary" size="sm">
          Play
        </Button>,
      ),
    );
    expect(classes).toContain("bg-accent");
    expect(classes).toContain("text-caption");
  });

  it("keeps the variant's colour token when the size sets a font size", () => {
    // `--text-on-accent` is what clears §2.1's contrast requirement on the accent, and the
    // size's `text-caption` sits in the same group as a colour to a merge library that has
    // not been told the scale. It was being dropped here, silently, until this test.
    const classes = classesOf(
      renderToStaticMarkup(
        <Button variant="primary" size="sm">
          Play
        </Button>,
      ),
    );
    expect(classes).toContain("text-text-on-accent");
    expect(classes).toContain("text-caption");
  });

  it("lets the call site win a conflicting utility", () => {
    const classes = classesOf(
      renderToStaticMarkup(
        <Button variant="primary" className="bg-surface-2">
          Play
        </Button>,
      ),
    );
    expect(classes).toContain("bg-surface-2");
    expect(classes).not.toContain("bg-accent");
  });

  it("renders the token-driven focus ring and reduced-motion branch", () => {
    const classes = classesOf(renderToStaticMarkup(<Button>Play</Button>));
    expect(classes).toContain("focus-visible:outline-focus-ring");
    expect(classes).toContain("motion-reduce:transition-none");
  });
});

describe("Button, destructive", () => {
  it("paints a destructive action in the danger token rather than the accent", () => {
    // A red button and an accent button must not look alike when one of them deletes something: the
    // confirmations AGENTS R22 requires are the callers that made this variant necessary.
    const markup = renderToStaticMarkup(<Button variant="danger">Forget</Button>);

    expect(markup).toContain("bg-state-danger");
    expect(markup).not.toContain("bg-accent");
  });
});
