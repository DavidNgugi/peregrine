import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { Switch } from "./Switch.js";

function control(markup: string): string {
  const at = markup.indexOf("<button");
  expect(at).toBeGreaterThan(-1);
  return markup.slice(at, markup.indexOf("</button>"));
}

const noop = (): void => undefined;

describe("Switch", () => {
  it("announces a state rather than an appearance", () => {
    const on = control(
      renderToStaticMarkup(<Switch checked label="Start when you sign in" onChange={noop} />),
    );
    expect(on).toContain('role="switch"');
    expect(on).toContain('aria-checked="true"');

    const off = control(
      renderToStaticMarkup(
        <Switch checked={false} label="Start when you sign in" onChange={noop} />,
      ),
    );
    expect(off).toContain('aria-checked="false"');
  });

  it("ties the label to the control so the words operate it", () => {
    const markup = renderToStaticMarkup(
      <Switch checked label="Start when you sign in" onChange={noop} />,
    );
    const id = /<button[^>]*id="([^"]*)"/u.exec(markup)?.[1] ?? "";
    expect(id).not.toBe("");
    expect(markup).toContain(`for="${id}"`);
  });

  it("clears the pointer target minimum even though the track is smaller", () => {
    // The visible track is 20 px, which is under §4's 28 px pointer minimum; the button
    // carries the padding that makes the target legal, and the label is a second target.
    const markup = control(
      renderToStaticMarkup(<Switch checked label="Start when you sign in" onChange={noop} />),
    );
    expect(markup).toContain("h-7");
  });

  it("hides the track from assistive technology, which the button already describes", () => {
    const markup = renderToStaticMarkup(
      <Switch checked label="Start when you sign in" onChange={noop} />,
    );
    expect(markup).toContain('aria-hidden="true"');
  });

  it("refuses input while a change is in flight, and says it is busy", () => {
    const markup = control(
      renderToStaticMarkup(<Switch busy checked label="Start when you sign in" onChange={noop} />),
    );
    expect(markup).toContain("disabled");
    expect(markup).toContain('aria-busy="true"');
  });

  it("refuses input when it is disabled, without claiming to be busy", () => {
    const markup = control(
      renderToStaticMarkup(
        <Switch checked disabled label="Start when you sign in" onChange={noop} />,
      ),
    );
    expect(markup).toContain("disabled");
    expect(markup).not.toContain("aria-busy");
  });

  it("renders the token focus ring and the reduced-motion branch", () => {
    const markup = renderToStaticMarkup(
      <Switch checked label="Start when you sign in" onChange={noop} />,
    );
    expect(markup).toContain("focus-visible:outline-focus-ring");
    expect(markup).toContain("motion-reduce:transition-none");
  });

  it("moves the thumb with one transform and no layout property", () => {
    const on = renderToStaticMarkup(<Switch checked label="A" onChange={noop} />);
    const off = renderToStaticMarkup(<Switch checked={false} label="A" onChange={noop} />);
    expect(on).toContain("translate-x-4");
    expect(off).toContain("translate-x-0");
  });
});
