import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { ShortcutKeys } from "./ShortcutKeys.js";

describe("ShortcutKeys", () => {
  it("renders the accelerator the core gave it, unchanged", () => {
    // Composing the string here would be a second rendering table, free to disagree with
    // the keys the shell actually dispatches on.
    const markup = renderToStaticMarkup(<ShortcutKeys display="⌥⇧⌘K" />);
    expect(markup).toContain("⌥⇧⌘K");
  });

  it("uses the element a keyboard input is announced by", () => {
    expect(renderToStaticMarkup(<ShortcutKeys display="⌘1" />)).toContain("<kbd");
  });

  it("draws a chip in the caption size and is not itself interactive", () => {
    const markup = renderToStaticMarkup(<ShortcutKeys display="⌘1" />);
    expect(markup).not.toContain("<button");
    expect(markup).toContain("font-mono");
    expect(markup).toContain("border-border-subtle");
    expect(markup).toContain("text-caption");
  });
});
