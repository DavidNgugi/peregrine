import { CATALOGUES } from "@peregrine/i18n";
import { describeIpcError, IPC_ERROR_CODES, type ErrorCode } from "@peregrine/ipc-client";
import { describe, expect, it } from "vitest";

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function lookup(source: unknown, path: string): unknown {
  return path
    .split(".")
    .reduce<unknown>((node, part) => (isRecord(node) ? node[part] : undefined), source);
}

function messagesFor(code: ErrorCode): readonly unknown[] {
  const copy = describeIpcError({ code, message: "", retryable: false });
  return [copy.titleKey, copy.adviceKey].map((key) =>
    lookup(CATALOGUES["en"], `translation.${key}`),
  );
}

// The mapping and the wording live in different packages, so nothing but this
// test stops a code from being added without its copy.
describe("IPC error copy", () => {
  it("resolves a non-empty message for every code the generated union declares", () => {
    expect(IPC_ERROR_CODES.length).toBeGreaterThan(0);
    for (const code of IPC_ERROR_CODES) {
      for (const message of messagesFor(code)) {
        expect(typeof message).toBe("string");
        expect(message).not.toBe("");
      }
    }
  });

  it("has a message for a code this build does not know yet", () => {
    // The core is newer than this build, so the code is not in the generated
    // union; the runtime fallback still has to land on real copy.
    const fromANewerCore = "FROM_A_NEWER_CORE" as ErrorCode;
    for (const message of messagesFor(fromANewerCore)) {
      expect(typeof message).toBe("string");
      expect(message).not.toBe("");
    }
  });
});
