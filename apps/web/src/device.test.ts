import { describe, expect, it } from "vitest";

import { describeDevice } from "./device.js";

const IPHONE_SAFARI =
  "Mozilla/5.0 (iPhone; CPU iPhone OS 18_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.5 Mobile/15E148 Safari/604.1";
const IPAD_DESKTOP_MODE =
  "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.5 Safari/605.1.15";
const ANDROID_TV =
  "Mozilla/5.0 (Linux; Android 12; BRAVIA 4K GB ATV3 Build/S3) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
const WINDOWS_EDGE =
  "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36 Edg/120.0.0.0";
const SAMSUNG_TV =
  "Mozilla/5.0 (SMART-TV; LINUX; Tizen 6.5) AppleWebKit/537.36 (KHTML, like Gecko) 85.0.4183.93/6.5 TV Safari/537.36";

describe("describeDevice", () => {
  it("names an iPhone and the browser it is running", () => {
    expect(
      describeDevice({
        userAgent: IPHONE_SAFARI,
        platform: "iPhone",
        maxTouchPoints: 5,
      }),
    ).toEqual({ name: "iPhone", platform: "Safari" });
  });

  it("reads an iPad in desktop mode from the touch count", () => {
    expect(
      describeDevice({
        userAgent: IPAD_DESKTOP_MODE,
        platform: "MacIntel",
        maxTouchPoints: 5,
      }),
    ).toEqual({ name: "iPad", platform: "Safari" });
  });

  it("tells an Android television from an Android phone", () => {
    expect(
      describeDevice({ userAgent: ANDROID_TV, platform: "Linux armv8l", maxTouchPoints: 0 }),
    ).toEqual({ name: "Android TV", platform: "Chrome" });
    expect(
      describeDevice({
        userAgent:
          "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Mobile Safari/537.36",
        platform: "Linux armv8l",
        maxTouchPoints: 5,
      }),
    ).toEqual({ name: "Android", platform: "Chrome" });
  });

  it("names a television by the platform it runs rather than its browser string", () => {
    expect(
      describeDevice({ userAgent: SAMSUNG_TV, platform: "Linux armv7l", maxTouchPoints: 0 }),
    ).toEqual({ name: "Smart TV", platform: "Tizen" });
  });

  it("reads Edge before the Chrome token it also carries", () => {
    expect(
      describeDevice({ userAgent: WINDOWS_EDGE, platform: "Win32", maxTouchPoints: 0 }),
    ).toEqual({ name: "Windows", platform: "Edge" });
  });

  it("falls back to a product name rather than an empty label", () => {
    expect(describeDevice({ userAgent: "", platform: "", maxTouchPoints: 0 })).toEqual({
      name: "Peregrine TV",
      platform: "Browser",
    });
  });
});
