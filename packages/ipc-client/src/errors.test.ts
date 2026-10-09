import { describe, expect, it } from "vitest";
import {
  describeIpcError,
  GENERIC_MESSAGE,
  IPC_ERROR_CODES,
  IPC_ERROR_COPY,
  isIpcErrorCode,
  normalizeIpcError,
  type ErrorCode,
  type IpcError,
} from "./index.js";

describe("describeIpcError", () => {
  it("has a distinct title and advice key for every code in the generated union", () => {
    // `IPC_ERROR_COPY` is annotated `Record<ErrorCode, ...>`, so a variant added
    // to the Rust enum fails this file at typecheck rather than silently rendering
    // the generic message at run time. That is the R4 test for the promise that no
    // code reaches the UI without copy.
    const keys = new Set<string>();
    for (const code of IPC_ERROR_CODES) {
      const copy = IPC_ERROR_COPY[code];
      expect(copy.titleKey).not.toBe("");
      expect(copy.adviceKey).not.toBe("");
      keys.add(copy.titleKey);
      keys.add(copy.adviceKey);
    }
    expect(keys.size).toBe(IPC_ERROR_CODES.length * 2);
  });

  it("knows every code in the generated union and nothing else", () => {
    for (const code of IPC_ERROR_CODES) {
      expect(isIpcErrorCode(code)).toBe(true);
    }
    expect(isIpcErrorCode("FROM_A_NEWER_CORE")).toBe(false);
    expect(isIpcErrorCode(7)).toBe(false);
  });

  it("gives a user-facing answer for a code this build does not know", () => {
    const fromANewerCore = "FROM_A_NEWER_CORE" as ErrorCode;
    const copy = describeIpcError({ code: fromANewerCore, message: "", retryable: false });
    expect(copy).toEqual(IPC_ERROR_COPY.INTERNAL);
  });

  it("does not overwrite the core's retry decision", () => {
    for (const code of IPC_ERROR_CODES) {
      expect(describeIpcError({ code, message: "m", retryable: true })).toEqual(
        IPC_ERROR_COPY[code],
      );
    }
  });
});

describe("normalizeIpcError", () => {
  it("keeps a well-formed envelope, detail and cause included", () => {
    const error = normalizeIpcError({
      code: "DISK_FULL",
      message: "no space left on device",
      detail: "1.2 GB required",
      cause: "ENOSPC",
      retryable: true,
    });
    expect(error).toEqual({
      code: "DISK_FULL",
      message: "no space left on device",
      detail: "1.2 GB required",
      cause: "ENOSPC",
      retryable: true,
    });
  });

  it("omits absent optional fields instead of setting them to undefined", () => {
    const error = normalizeIpcError({ code: "NOT_FOUND", message: "gone", retryable: false });
    expect(Object.hasOwn(error, "detail")).toBe(false);
    expect(Object.hasOwn(error, "cause")).toBe(false);
  });

  it("treats a missing or empty code as internal and never leaves a blank message", () => {
    expect(normalizeIpcError({ message: "unknown code" }).code).toBe("INTERNAL");
    expect(normalizeIpcError({ code: "", message: "unknown code" }).code).toBe("INTERNAL");
    expect(normalizeIpcError({ code: "INTERNAL", message: "" }).message).toBe(GENERIC_MESSAGE);
    expect(normalizeIpcError({}).message).toBe(GENERIC_MESSAGE);
  });

  it("does not offer a retry the core did not promise", () => {
    expect(normalizeIpcError({ code: "PROVIDER_UNAVAILABLE" }).retryable).toBe(false);
    expect(normalizeIpcError({ code: "PROVIDER_UNAVAILABLE", retryable: "yes" }).retryable).toBe(
      false,
    );
  });

  it("drops non-string detail and cause rather than stringifying them", () => {
    const error = normalizeIpcError({
      code: "INTERNAL",
      message: "boom",
      detail: { nested: true },
      cause: new Error("cause"),
      retryable: false,
    });
    expect(Object.hasOwn(error, "detail")).toBe(false);
    expect(Object.hasOwn(error, "cause")).toBe(false);
  });

  it("normalizes a plain Error and a bare string", () => {
    expect(normalizeIpcError(new Error("dashboard exploded"))).toEqual({
      code: "INTERNAL",
      message: "dashboard exploded",
      retryable: false,
    });
    expect(normalizeIpcError("plain string")).toEqual({
      code: "INTERNAL",
      message: "plain string",
      retryable: false,
    });
  });

  it("survives values that are not errors at all", () => {
    const expected: IpcError = {
      code: "INTERNAL",
      message: GENERIC_MESSAGE,
      retryable: false,
    };
    for (const value of [null, undefined, 42, [], "", "   ", Symbol("nope")]) {
      expect(normalizeIpcError(value)).toEqual(expected);
    }
  });

  it("does not throw when reading the envelope throws", () => {
    const hostile = {
      get code(): string {
        throw new Error("hostile accessor");
      },
    };
    const error = normalizeIpcError(hostile);
    expect(error.code).toBe("INTERNAL");
    expect(error.retryable).toBe(false);
  });
});
