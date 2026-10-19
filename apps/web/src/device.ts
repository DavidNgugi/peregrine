/**
 * The label a browser can honestly give about itself at pairing.
 *
 * A web page cannot read the device's name: no API exposes it, and an iPad in desktop mode
 * reports a Macintosh user agent, which is why the platform and the touch count are consulted
 * alongside the user agent. The result names the kind of device, not the person's device; the
 * person renames it from the desktop if they want something else.
 */
export interface DeviceHints {
  readonly userAgent: string;
  readonly platform: string;
  readonly maxTouchPoints: number;
}

/** What a paired device is called, and what it is running. */
export interface DeviceLabel {
  readonly name: string;
  readonly platform: string;
}

const BROWSERS: readonly (readonly [RegExp, string])[] = [
  [/Edg\//, "Edge"],
  [/OPR\//, "Opera"],
  [/Firefox\//, "Firefox"],
  [/CriOS\//, "Chrome"],
  [/Chrome\//, "Chrome"],
  [/Safari\//, "Safari"],
];

const DEVICES: readonly (readonly [RegExp, string])[] = [
  [/iPhone/, "iPhone"],
  [/iPad/, "iPad"],
  [/AppleTV|Apple TV/, "Apple TV"],
  [/Windows/, "Windows"],
  [/Macintosh|Mac OS X/, "Mac"],
  [/Linux/, "Linux"],
];

/** The name and browser a client offers when it pairs. */
export function describeDevice(hints: DeviceHints): DeviceLabel {
  const { userAgent, platform, maxTouchPoints } = hints;
  // A television's browser string is a poor label; the platform it runs is the useful half.
  const television = /Tizen/.test(userAgent)
    ? "Tizen"
    : /Web0S|webOS/.test(userAgent)
      ? "webOS"
      : null;
  if (television !== null) {
    return { name: "Smart TV", platform: television };
  }
  // iPadOS reports a Macintosh user agent; the touch count is the part that still says iPad.
  const name = platform === "MacIntel" && maxTouchPoints > 1 ? "iPad" : deviceName(userAgent);
  return { name, platform: describe(userAgent, BROWSERS) ?? "Browser" };
}

function deviceName(userAgent: string): string {
  if (/Android/.test(userAgent)) {
    return /TV|AFT/.test(userAgent) ? "Android TV" : "Android";
  }
  return describe(userAgent, DEVICES) ?? "Peregrine TV";
}

function describe(
  userAgent: string,
  table: readonly (readonly [RegExp, string])[],
): string | undefined {
  return table.find(([pattern]) => pattern.test(userAgent))?.[1];
}
