import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { ICON_NAMES, Icon } from "./Icon.js";

function classesOf(markup: string): string[] {
  const match = /class="([^"]*)"/u.exec(markup);
  return match?.[1]?.split(/\s+/u) ?? [];
}

describe("Icon", () => {
  it("draws every icon in the set on the shared grid with the shared stroke", () => {
    // The family, not a sample: §9 says one grid and one stroke width, and an icon that
    // quietly differs is what makes a set look assembled rather than drawn.
    for (const name of ICON_NAMES) {
      const markup = renderToStaticMarkup(<Icon name={name} />);
      expect(markup, name).toContain('viewBox="0 0 24 24"');
      expect(markup, name).toContain('stroke-width="1.5"');
      expect(markup, name).toContain('fill="none"');
      expect(markup, name).toMatch(/<(path|circle)/u);
    }
  });

  it("draws every icon in the set as decorative geometry rather than as a named image", () => {
    // The name belongs to the control around the icon; an `svg` that also names itself
    // announces the same thing twice (§9).
    for (const name of ICON_NAMES) {
      const markup = renderToStaticMarkup(<Icon name={name} />);
      expect(markup, name).toContain('aria-hidden="true"');
      expect(markup, name).toContain('focusable="false"');
      expect(markup, name).not.toContain("role=");
      expect(markup, name).not.toContain("<title");
    }
  });

  it("takes its colour from the text around it and never from the token layer", () => {
    const markup = renderToStaticMarkup(<Icon className="text-accent" name="moon" />);
    expect(markup).toContain('stroke="currentColor"');
    // A colour baked into the primitive would fight the caller's, so the only colour
    // class in the output is the one the call site passed.
    expect(classesOf(markup)).toContain("text-accent");
  });

  it("draws at the documented size by default and at each size §9 allows", () => {
    expect(renderToStaticMarkup(<Icon name="moon" />)).toContain('height="20"');
    for (const size of [16, 24, 32] as const) {
      const markup = renderToStaticMarkup(<Icon name="moon" size={size} />);
      expect(markup, String(size)).toContain(`height="${String(size)}"`);
      expect(markup, String(size)).toContain(`width="${String(size)}"`);
    }
  });

  it("draws geometry and never a glyph: no text element, no emoji (§9)", () => {
    // The whole set is walked, and the printable-ASCII bound is what makes the emoji half
    // of the rule mechanical: §9 bans emoji as icons, and an emoji in the markup would be
    // a non-ASCII character in the output.
    expect(ICON_NAMES.length).toBeGreaterThan(1);
    for (const name of ICON_NAMES) {
      const markup = renderToStaticMarkup(<Icon name={name} />);
      expect(markup, name).not.toContain("<text");
      expect(markup, name).toMatch(/^[\x20-\x7E]*$/u);
    }
  });
});
