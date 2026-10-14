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

  it("can be drawn as a button without becoming one", () => {
    // Discover's hero puts a Play button and a Details link side by side, and the link has to match
    // the control beside it. It stays an anchor — the destination is in the markup, which is the
    // whole reason the kit has a `Link` — and its classes come from `buttonVariants`, so the two
    // cannot drift apart.
    const plain = renderToStaticMarkup(<Link href="/library">Library</Link>);
    const asButton = renderToStaticMarkup(
      <Link href="/library" variant="secondary">
        Library
      </Link>,
    );

    expect(asButton).toContain("<a ");
    expect(asButton).not.toContain("<button");
    expect(asButton).toContain('href="/library"');
    expect(asButton).toContain("border-border-subtle");
    expect(asButton).toContain("h-9");
    expect(asButton).not.toContain("hover:underline");
    expect(plain).not.toContain("border-border-subtle");
    expect(plain).toContain("hover:underline");
  });

  it("takes the size a button takes", () => {
    const small = renderToStaticMarkup(
      <Link href="/library" size="sm" variant="primary">
        Library
      </Link>,
    );

    expect(small).toContain("h-7");
    expect(small).toContain("bg-accent");
  });
});
