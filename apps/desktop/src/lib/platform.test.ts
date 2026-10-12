import { describe, expect, it } from "vitest";
import { parsePlatform, resolvePlatform } from "./platform.js";

describe("parsePlatform", () => {
  it("reads the OS segment of a BuildInfo target", () => {
    expect(parsePlatform("macos-aarch64")).toBe("macos");
    expect(parsePlatform("windows-x86_64")).toBe("windows");
    expect(parsePlatform("linux-x86_64")).toBe("linux");
  });

  it("reads a target with no architecture segment", () => {
    expect(parsePlatform("macos")).toBe("macos");
  });

  it("answers null for anything it has no chrome for, rather than guessing", () => {
    expect(parsePlatform("")).toBeNull();
    expect(parsePlatform("-aarch64")).toBeNull();
    expect(parsePlatform("freebsd-x86_64")).toBeNull();
    expect(parsePlatform("macosx-aarch64")).toBeNull();
    // Exact match: `std::env::consts::OS` is lower case, so an upper-case value means
    // something changed at the boundary and a silent match would hide it.
    expect(parsePlatform("MACOS-aarch64")).toBeNull();
  });
});

describe("resolvePlatform", () => {
  // No Tauri API is mocked here: outside the shell the read is expected to report that it
  // cannot answer, which is what keeps a browser preview of the frontend rendering.
  it("answers null when there is no shell to ask", async () => {
    expect(globalThis).not.toHaveProperty("isTauri");
    await expect(resolvePlatform()).resolves.toBeNull();
  });
});
