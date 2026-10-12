import { beforeEach, describe, expect, it, vi } from "vitest";

// The plugin is named only through its own module, so the test replaces that module and
// drives the typed wrapper in Node. `isTauri` comes from `@tauri-apps/api/core`.
const plugin = vi.hoisted(() => ({
  isPermissionGranted: vi.fn(async () => true),
  requestPermission: vi.fn(async () => "granted" as NotificationPermission),
  tauri: true,
}));

vi.mock("@tauri-apps/api/core", () => ({
  isTauri: () => plugin.tauri,
}));

vi.mock("@tauri-apps/plugin-notification", () => ({
  isPermissionGranted: plugin.isPermissionGranted,
  requestPermission: plugin.requestPermission,
}));

const { notificationSettingsUrl, probePermission, requestPermission } =
  await import("./notifications.js");

beforeEach(() => {
  plugin.isPermissionGranted.mockClear();
  plugin.requestPermission.mockClear();
  plugin.isPermissionGranted.mockResolvedValue(true);
  plugin.requestPermission.mockResolvedValue("granted");
  plugin.tauri = true;
});

describe("probePermission", () => {
  it("reports a granted system permission without prompting", async () => {
    await expect(probePermission()).resolves.toEqual({ kind: "granted" });
    expect(plugin.isPermissionGranted).toHaveBeenCalledTimes(1);
    expect(plugin.requestPermission).not.toHaveBeenCalled();
  });

  it("reports a refused permission", async () => {
    plugin.isPermissionGranted.mockResolvedValue(false);
    await expect(probePermission()).resolves.toEqual({ kind: "denied" });
  });

  it("never claims a permission outside the Tauri shell", async () => {
    plugin.tauri = false;
    await expect(probePermission()).resolves.toEqual({ kind: "unverified" });
    expect(plugin.isPermissionGranted).not.toHaveBeenCalled();
  });

  it("reports unverified rather than granted when the probe throws", async () => {
    plugin.isPermissionGranted.mockRejectedValue(new Error("no bridge"));
    await expect(probePermission()).resolves.toEqual({ kind: "unverified" });
  });
});

describe("requestPermission", () => {
  it("reports the answer the system gave", async () => {
    plugin.requestPermission.mockResolvedValue("granted");
    await expect(requestPermission()).resolves.toEqual({ kind: "granted" });

    plugin.requestPermission.mockResolvedValue("denied");
    await expect(requestPermission()).resolves.toEqual({ kind: "denied" });
  });

  it("treats a dismissed prompt as unverified, never as granted", async () => {
    plugin.requestPermission.mockResolvedValue("default");
    await expect(requestPermission()).resolves.toEqual({ kind: "unverified" });
  });

  it("reports unverified outside the shell and when the ask throws", async () => {
    plugin.tauri = false;
    await expect(requestPermission()).resolves.toEqual({ kind: "unverified" });
    expect(plugin.requestPermission).not.toHaveBeenCalled();

    plugin.tauri = true;
    plugin.requestPermission.mockRejectedValue(new Error("no bridge"));
    await expect(requestPermission()).resolves.toEqual({ kind: "unverified" });
  });
});

describe("notificationSettingsUrl", () => {
  it("names each platform's own settings page", () => {
    expect(notificationSettingsUrl("macos")).toBe(
      "x-apple.systempreferences:com.apple.Notifications-Settings.extension",
    );
    expect(notificationSettingsUrl("windows")).toBe("ms-settings:notifications");
  });

  it("offers no link where the platform has no portable page", () => {
    expect(notificationSettingsUrl("linux")).toBeNull();
    expect(notificationSettingsUrl(null)).toBeNull();
  });
});
