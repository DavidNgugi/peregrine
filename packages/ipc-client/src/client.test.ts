import { beforeEach, describe, expect, it, vi } from "vitest";

// The host app supplies the Tauri bridge, so the test replaces exactly that
// module and drives the generated `commands.buildInfo()` call site in Node.
const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

const { getBuildInfo, homeRails, searchProviders, isSuccessful, unwrap } =
  await import("./client.js");

beforeEach(() => {
  invoke.mockReset();
});

describe("getBuildInfo", () => {
  it("returns the generated DTO from the bound command", async () => {
    const info = { appName: "Peregrine", appVersion: "0.1.0", target: "macos-aarch64" };
    invoke.mockResolvedValueOnce(info);

    await expect(getBuildInfo()).resolves.toEqual(info);
    expect(invoke).toHaveBeenCalledWith("build_info");
  });

  it("throws the shared error shape when the command answers with an error", async () => {
    invoke.mockRejectedValueOnce({
      code: "PERMISSION_DENIED",
      message: "Peregrine does not have permission to do that.",
      retryable: false,
    });

    await expect(getBuildInfo()).rejects.toEqual({
      code: "PERMISSION_DENIED",
      message: "Peregrine does not have permission to do that.",
      retryable: false,
    });
  });

  it("falls back to INTERNAL for a code this build does not know", async () => {
    invoke.mockRejectedValueOnce({ code: "FROM_A_NEWER_CORE", message: "no", retryable: false });

    await expect(getBuildInfo()).rejects.toMatchObject({ code: "INTERNAL" });
  });
});

describe("isSuccessful", () => {
  it("narrows the generated result union", () => {
    const ok = { status: "ok", data: 1 } as const;
    const error = { status: "error", error: "boom" } as const;

    expect(isSuccessful(ok)).toBe(true);
    expect(isSuccessful(error)).toBe(false);
    if (isSuccessful(ok)) {
      expect(ok.data).toBe(1);
    }
  });
});

describe("unwrap", () => {
  it("returns the payload of a successful result", async () => {
    await expect(unwrap(Promise.resolve({ status: "ok", data: { id: 1 } }))).resolves.toEqual({
      id: 1,
    });
  });

  it("throws the normalized error of a failed result", async () => {
    const failure = {
      status: "error",
      error: { code: "NOT_FOUND", message: "gone", retryable: false },
    } as const;
    await expect(unwrap(Promise.resolve(failure))).rejects.toMatchObject({ code: "NOT_FOUND" });
  });

  it("throws INTERNAL for a code this build does not know", async () => {
    const failure = {
      status: "error",
      error: { code: "FROM_A_NEWER_CORE", message: "no", retryable: false },
    } as const;
    await expect(unwrap(Promise.resolve(failure))).rejects.toMatchObject({ code: "INTERNAL" });
  });
});

describe("Discover title type", () => {
  it("sends the same title type to rails and search", async () => {
    invoke.mockResolvedValue([]);
    await homeRails("show");
    await searchProviders("Alien", 10, "show");
    expect(invoke).toHaveBeenCalledWith("home_rails", { kind: "show" });
    expect(invoke).toHaveBeenCalledWith("search_providers", {
      query: "Alien",
      limit: 10,
      kind: "show",
    });
  });
  it("retains the unfiltered read for callers without a title type", async () => {
    invoke.mockResolvedValue([]);
    await homeRails();
    await searchProviders("Alien");
    expect(invoke).toHaveBeenCalledWith("home_rails", { kind: null });
    expect(invoke).toHaveBeenCalledWith("search_providers", {
      query: "Alien",
      limit: null,
      kind: null,
    });
  });
});
