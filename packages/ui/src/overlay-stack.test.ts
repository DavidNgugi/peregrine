import { describe, expect, it } from "vitest";
import { createOverlayStack } from "./overlay-stack.js";

describe("overlay stack", () => {
  it("answers that the last one opened is on top", () => {
    const stack = createOverlayStack();

    stack.push("screen-dialog");
    stack.push("confirmation");

    expect(stack.isTop("confirmation")).toBe(true);
    expect(stack.isTop("screen-dialog")).toBe(false);
    expect(stack.top()).toBe("confirmation");
  });

  it("gives the top back to its parent when a nested overlay closes", () => {
    // The rule the module exists for: a confirmation closing puts the user back in the dialog that
    // opened it rather than out of it.
    const stack = createOverlayStack();
    stack.push("screen-dialog");
    stack.push("confirmation");

    stack.remove("confirmation");

    expect(stack.isTop("screen-dialog")).toBe(true);
    expect(stack.top()).toBe("screen-dialog");
  });

  it("lets Escape close only the top when two are open", () => {
    // Stated as the interaction rather than as the state: two overlays are open, both ask the stack
    // whether a key press is theirs, and exactly one says yes.
    const stack = createOverlayStack();
    stack.push("first");
    stack.push("second");

    const answered = ["first", "second"].filter((id) => stack.isTop(id));

    expect(answered).toEqual(["second"]);
  });

  it("does not stack one overlay twice", () => {
    // A component that re-renders must not make Escape need pressing twice.
    const stack = createOverlayStack();

    stack.push("dialog");
    stack.push("dialog");

    expect(stack.size()).toBe(1);
    expect(stack.isTop("dialog")).toBe(true);
  });

  it("moves an overlay back to the top when it is pushed again", () => {
    // A dialog that was open, then covered, then re-focused is the one a key belongs to.
    const stack = createOverlayStack();
    stack.push("first");
    stack.push("second");

    stack.push("first");

    expect(stack.top()).toBe("first");
    expect(stack.size()).toBe(2);
  });

  it("is empty-safe", () => {
    const stack = createOverlayStack();

    expect(stack.top()).toBeNull();
    expect(stack.isTop("anything")).toBe(false);
    expect(stack.size()).toBe(0);
    expect(() => {
      stack.remove("never-opened");
    }).not.toThrow();
  });

  it("keeps each stack to itself", () => {
    // A factory rather than one module-level array: a test that left something open must not change
    // what another test sees, and neither may two windows in a future multi-window build.
    const left = createOverlayStack();
    const right = createOverlayStack();

    left.push("left-only");

    expect(right.size()).toBe(0);
    expect(right.top()).toBeNull();
  });
});
