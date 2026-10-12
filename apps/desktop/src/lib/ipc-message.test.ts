import { describe, expect, it } from "vitest";
import { messageOf } from "./ipc-message.js";

/** A failed call as the shared mapper produces one. */
function error(overrides: Partial<Parameters<typeof messageOf>[0]> = {}) {
  return { code: "CONFLICT", message: "conflict", retryable: false, ...overrides } as const;
}

describe("messageOf", () => {
  it("prefers the sentence the core wrote for a person", () => {
    expect(messageOf(error({ detail: "A scan is already running." }))).toBe(
      "A scan is already running.",
    );
  });

  it("falls back to the code's own word when the core sent no sentence", () => {
    expect(messageOf(error())).toBe("conflict");
  });

  it("always answers with something", () => {
    // The last resort is a catalogue key rather than an empty string: a message box with nothing in
    // it is worse than one naming the failure's family.
    expect(messageOf(error({ message: "" }))).not.toBe("");
  });
});
