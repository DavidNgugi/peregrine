import { describe, expect, it } from "vitest";
import {
  describeIpcError,
  GENERIC_MESSAGE,
  IPC_ERROR_CODES,
  IPC_ERROR_COPY,
  normalizeIpcError,
  type IpcError,
  type IpcErrorCode,
} from "./index.js";

describe("describeIpcError", () => {
  it("maps every known code to a distinct title and advice key", () => {
    const keys = new Set<string>();
    for (const code of IPC_ERROR_CODES) {
      const copy = IPC_ERROR_COPY[code];
      expect(copy.titleKey).toBe(`ipc.error.${code}.title`);
      expect(copy.adviceKey).toBe(`ipc.error.${code}.advice`);
      keys.add(copy.titleKey);
      keys.add(copy.adviceKey);
    }
    expect(keys.size).toBe(IPC_ERROR_CODES.length * 2);
  });

  it("gives a user-facing answer for a code this build does not know", () => {
    const copy = describeIpcError({ code: "not_a_real_code", message: "", retryable: false });
    expect(copy).toEqual(IPC_ERROR_COPY.internal);
  });

  it("does not overwrite the core's retry decision", () => {
    const codes: IpcErrorCode[] = [...IPC_ERROR_CODES];
    for (const code of codes) {
      expect(describeIpcError({ code, message: "m", retryable: true })).toEqual(
        IPC_ERROR_COPY[code],
      );
    }
  });
});

describe("normalizeIpcError", () => {
  it("keeps a well-formed envelope, detail and cause included", () => {
    const error = normalizeIpcError({
      code: "disk_full",
      message: "no space left on device",
      detail: "1.2 GB required",
      cause: "ENOSPC",
      retryable: true,
    });
    expect(error).toEqual({
      code: "disk_full",
      message: "no space left on device",
      detail: "1.2 GB required",
      cause: "ENOSPC",
      retryable: true,
    });
  });

  it("omits absent optional fields instead of setting them to undefined", () => {
    const error = normalizeIpcError({ code: "not_found", message: "gone", retryable: false });
    expect(Object.hasOwn(error, "detail")).toBe(false);
    expect(Object.hasOwn(error, "cause")).toBe(false);
  });

  it("treats a missing or empty code as internal and never leaves a blank message", () => {
    expect(normalizeIpcError({ message: "unknown code" }).code).toBe("internal");
    expect(normalizeIpcError({ code: "", message: "unknown code" }).code).toBe("internal");
    expect(normalizeIpcError({ code: "internal", message: "" }).message).toBe(GENERIC_MESSAGE);
    expect(normalizeIpcError({}).message).toBe(GENERIC_MESSAGE);
  });

  it("does not offer a retry the core did not promise", () => {
    expect(normalizeIpcError({ code: "provider_unavailable" }).retryable).toBe(false);
    expect(normalizeIpcError({ code: "provider_unavailable", retryable: "yes" }).retryable).toBe(
      false,
    );
  });

  it("drops non-string detail and cause rather than stringifying them", () => {
    const error = normalizeIpcError({
      code: "internal",
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
      code: "internal",
      message: "dashboard exploded",
      retryable: false,
    });
    expect(normalizeIpcError("plain string")).toEqual({
      code: "internal",
      message: "plain string",
      retryable: false,
    });
  });

  it("survives values that are not errors at all", () => {
    const expected: IpcError = {
      code: "internal",
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
    expect(error.code).toBe("internal");
    expect(error.retryable).toBe(false);
  });
});
