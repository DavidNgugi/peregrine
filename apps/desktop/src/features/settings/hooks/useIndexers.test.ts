import { describe, expect, it, vi } from "vitest";

import { RETRY_DELAYS_MS, retrying } from "./useIndexers.js";

const unavailable = {
  code: "PROVIDER_UNAVAILABLE",
  message: "unavailable",
  retryable: true,
} as const;
const internal = { code: "INTERNAL", message: "internal", retryable: false } as const;

describe("retrying", () => {
  it("answers once a Prowlarr that was still starting comes up", async () => {
    const read = vi
      .fn<() => Promise<string>>()
      .mockRejectedValueOnce(unavailable)
      .mockRejectedValueOnce(unavailable)
      .mockResolvedValue("ready");
    const sleep = vi.fn(async () => undefined);

    await expect(retrying(read, sleep)).resolves.toBe("ready");
    expect(read).toHaveBeenCalledTimes(3);
    expect(sleep.mock.calls).toEqual([[RETRY_DELAYS_MS[0]], [RETRY_DELAYS_MS[1]]]);
  });

  it("does not retry a failure the core calls permanent", async () => {
    const read = vi.fn<() => Promise<string>>().mockRejectedValue(internal);
    const sleep = vi.fn(async () => undefined);

    await expect(retrying(read, sleep)).rejects.toEqual(internal);
    expect(read).toHaveBeenCalledTimes(1);
    expect(sleep).not.toHaveBeenCalled();
  });

  it("gives up after the last delay", async () => {
    const read = vi.fn<() => Promise<string>>().mockRejectedValue(unavailable);
    const sleep = vi.fn(async () => undefined);

    await expect(retrying(read, sleep)).rejects.toEqual(unavailable);
    expect(read).toHaveBeenCalledTimes(RETRY_DELAYS_MS.length + 1);
    expect(sleep).toHaveBeenCalledTimes(RETRY_DELAYS_MS.length);
  });
});
