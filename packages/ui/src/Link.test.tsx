import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { Link, shouldNavigate } from "./Link.js";

/** A click event, as much of one as the decision reads. */
function click(overrides: Partial<Record<string, boolean>> = {}): {
  readonly event: Parameters<typeof shouldNavigate>[0];
  readonly prevented: () => boolean;
} {
  let prevented = false;
  const event = {
    altKey: false,
    ctrlKey: false,
    metaKey: false,
    shiftKey: false,
    defaultPrevented: false,
    preventDefault: () => {
      prevented = true;
    },
    ...overrides,
  } as unknown as Parameters<typeof shouldNavigate>[0];
  return { event, prevented: () => prevented };
}

describe("Link", () => {
  it("carries its destination in the markup", () => {
    // What a context menu, a copy, and a screen reader read. A button that navigates carries none of
    // it, which is what the library screen had.
    const markup = renderToStaticMarkup(<Link href="/title/movie/7">Alien</Link>);

    expect(markup).toContain('href="/title/movie/7"');
    expect(markup).toContain("Alien");
    expect(markup).toContain("<a ");
  });

  it("takes the click instead of reloading the application", () => {
    // Which in a webview is the difference between moving to the next screen and reloading the app.
    expect(shouldNavigate(click().event, true)).toBe(true);
  });

  it("leaves a modified click to the platform", () => {
    // `Cmd`-click is a request for a new window, and taking it would be the app deciding it knows
    // better than the platform convention.
    for (const modifier of ["metaKey", "ctrlKey", "shiftKey", "altKey"]) {
      expect(shouldNavigate(click({ [modifier]: true }).event, true), modifier).toBe(false);
    }
  });

  it("behaves as it is written when the caller gives no navigation", () => {
    // An anchor with no handler is an anchor: that is what an external link is, and what the rendered
    // markup asserts.
    expect(shouldNavigate(click().event, false)).toBe(false);
  });

  it("respects another handler's decision", () => {
    // A caller that prevents the click itself — a confirmation, a guard — is not overruled by the
    // link's own navigation.
    expect(shouldNavigate(click({ defaultPrevented: true }).event, true)).toBe(false);
  });

  it("keeps the platform's focus ring rather than the browser's outline", () => {
    const markup = renderToStaticMarkup(<Link href="/library">Library</Link>);

    expect(markup).toContain("focus-visible:outline-focus-ring");
  });
});
