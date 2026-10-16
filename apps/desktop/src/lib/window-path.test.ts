import { afterEach, describe, expect, it, vi } from "vitest";
import { writePath } from "./window-path.js";

describe("writePath", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("does nothing without a window rather than throwing", () => {
    // The Node test environment and any future server-side render have no history object. A throw
    // here would be a screen that cannot render without a browser, which is what the entry's own
    // split exists to avoid.
    expect(() => {
      writePath("/library");
    }).not.toThrow();
  });

  it("replaces the path without adding a history entry", () => {
    // Replace rather than push: the shell keeps its own history (`app/navigation.ts`), and a
    // browser stack that grew with every sidebar click would make the OS's own back gesture walk
    // through them.
    const replaceState = vi.fn();
    vi.stubGlobal("window", { history: { replaceState } });

    writePath("/settings");

    expect(replaceState).toHaveBeenCalledWith(null, "", "/settings");
  });
});
