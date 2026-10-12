import { describe, expect, it } from "vitest";
import type { ShortcutBinding } from "@peregrine/ipc-client";
import { shortcutFor } from "./shortcut-dispatch.js";

/** A binding as the core sends it on a macOS build, where command_or_control is Meta. */
function binding(overrides: Partial<ShortcutBinding> = {}): ShortcutBinding {
  return {
    id: "navigation.discover",
    labelKey: "shortcuts.discover",
    display: "⌘1",
    code: "Digit1",
    meta: true,
    ctrl: false,
    alt: false,
    shift: false,
    ...overrides,
  };
}

describe("shortcutFor", () => {
  it("matches the key and every modifier the binding names", () => {
    const bindings = [binding()];
    expect(
      shortcutFor(bindings, {
        code: "Digit1",
        metaKey: true,
        ctrlKey: false,
        altKey: false,
        shiftKey: false,
      })?.id,
    ).toBe("navigation.discover");
  });

  it("does not fire when a modifier the user added is held", () => {
    expect(
      shortcutFor([binding()], {
        code: "Digit1",
        metaKey: true,
        ctrlKey: false,
        altKey: false,
        shiftKey: true,
      }),
    ).toBeNull();
  });

  it("does not fire when a modifier the binding wants is missing", () => {
    expect(
      shortcutFor([binding()], {
        code: "Digit1",
        metaKey: false,
        ctrlKey: false,
        altKey: false,
        shiftKey: false,
      }),
    ).toBeNull();
  });

  it("does not fire on the right key with the wrong modifier key", () => {
    // The macOS build resolves command-or-control to Meta, so Control alone is a
    // different shortcut rather than the same one (the classic macOS/Windows mix-up).
    expect(
      shortcutFor([binding()], {
        code: "Digit1",
        metaKey: false,
        ctrlKey: true,
        altKey: false,
        shiftKey: false,
      }),
    ).toBeNull();
  });

  it("answers null for a key nothing is bound to, and for an empty registry", () => {
    const press = {
      code: "Digit9",
      metaKey: true,
      ctrlKey: false,
      altKey: false,
      shiftKey: false,
    };
    expect(shortcutFor([binding()], press)).toBeNull();
    expect(shortcutFor([], { ...press, code: "Digit1" })).toBeNull();
  });

  it("tells two bindings apart when only the key differs", () => {
    const bindings = [binding(), binding({ id: "navigation.jobs", code: "Digit2" })];
    expect(
      shortcutFor(bindings, {
        code: "Digit2",
        metaKey: true,
        ctrlKey: false,
        altKey: false,
        shiftKey: false,
      })?.id,
    ).toBe("navigation.jobs");
  });
});
