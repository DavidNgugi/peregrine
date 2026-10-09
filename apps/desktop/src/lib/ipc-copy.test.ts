import { CATALOGUES } from "@peregrine/i18n";
import { describeIpcError, IPC_ERROR_CODES } from "@peregrine/ipc-client";
import { describe, expect, it } from "vitest";

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function lookup(source: unknown, path: string): unknown {
  return path
    .split(".")
    .reduce<unknown>((node, part) => (isRecord(node) ? node[part] : undefined), source);
}

function messagesFor(code: string): readonly unknown[] {
  const copy = describeIpcError({ code, message: "", retryable: false });
  return [copy.titleKey, copy.adviceKey].map((key) =>
    lookup(CATALOGUES["en"], `translation.${key}`),
  );
}

// The mapping and the wording live in different packages, so nothing but this
// test stops a code from being added without its copy.
describe("IPC error copy", () => {
  it("has a non-empty message for every known code", () => {
    for (const code of IPC_ERROR_CODES) {
      for (const message of messagesFor(code)) {
        expect(typeof message).toBe("string");
        expect(message).not.toBe("");
      }
    }
  });

  it("has a message for a code this build does not know yet", () => {
    for (const message of messagesFor("a_code_from_a_newer_core")) {
      expect(typeof message).toBe("string");
      expect(message).not.toBe("");
    }
  });
});
