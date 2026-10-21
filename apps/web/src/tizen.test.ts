import { describe, expect, it } from "vitest";

import {
  BACK_KEY_CODE,
  backTarget,
  focusPlateBox,
  isBackKey,
  isHardwareBack,
  isTizen,
  transportAction,
  type ClientScreen,
} from "./tizen.js";

const TIZEN_6 =
  "Mozilla/5.0 (SMART-TV; LINUX; Tizen 6.0) AppleWebKit/537.36 (KHTML, like Gecko) 76.0.3809.146/6.0 TV Safari/537.36";

describe("isTizen", () => {
  it("recognises the television and not the desktop or phone", () => {
    expect(isTizen(TIZEN_6)).toBe(true);
    expect(isTizen("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)")).toBe(false);
    expect(isTizen("Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X)")).toBe(false);
  });
});

describe("Back", () => {
  it("accepts the hardware event and the fallback key code", () => {
    expect(isHardwareBack("back")).toBe(true);
    expect(isHardwareBack("menu")).toBe(false);
    expect(isHardwareBack(undefined)).toBe(false);
    expect(isBackKey(BACK_KEY_CODE)).toBe(true);
    expect(isBackKey(13)).toBe(false);
  });

  it("leaves the app at the root and pops history everywhere else", () => {
    const roots: ClientScreen[] = ["loading", "pair", "home"];
    const inner: ClientScreen[] = ["browse", "myList", "downloads", "library", "title"];
    for (const screen of roots) expect(backTarget(screen), screen).toBe("exit");
    for (const screen of inner) expect(backTarget(screen), screen).toBe("back");
    expect(backTarget("player")).toBe("player");
  });
});

describe("transportAction", () => {
  it("maps the Tizen key codes", () => {
    expect(transportAction(415)).toBe("play");
    expect(transportAction(19)).toBe("pause");
    expect(transportAction(10_252)).toBe("toggle");
    expect(transportAction(413)).toBe("stop");
    expect(transportAction(417)).toBe("forward");
    expect(transportAction(412)).toBe("rewind");
  });

  it("accepts the DOM key names when the build sends one", () => {
    expect(transportAction(0, "MediaPlayPause")).toBe("toggle");
    expect(transportAction(0, "MediaPlay")).toBe("play");
    expect(transportAction(0, "MediaPause")).toBe("pause");
    expect(transportAction(0, "MediaStop")).toBe("stop");
    expect(transportAction(0, "MediaFastForward")).toBe("forward");
    expect(transportAction(0, "MediaRewind")).toBe("rewind");
  });

  it("leaves D-pad, OK, and Back to their own handlers", () => {
    for (const keyCode of [13, 37, 38, 39, 40, BACK_KEY_CODE]) {
      expect(transportAction(keyCode)).toBeNull();
    }
    expect(transportAction(0, "Enter")).toBeNull();
  });
});

describe("focusPlateBox", () => {
  it("surrounds the target by the ring inset on every side", () => {
    expect(focusPlateBox({ x: 100, y: 50, width: 200, height: 300 }, 4)).toEqual({
      x: 96,
      y: 46,
      width: 208,
      height: 308,
    });
  });

  it("uses the default inset and keeps the box coherent at the viewport edge", () => {
    expect(focusPlateBox({ x: 0, y: 0, width: 10, height: 20 })).toEqual({
      x: -4,
      y: -4,
      width: 18,
      height: 28,
    });
  });
});
