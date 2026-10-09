import { beforeEach, describe, expect, it } from "vitest";
import { resolveTheme, themeStore } from "./theme-store.js";

const initial = {
  preference: "system",
  systemTheme: null,
  resolved: "dark",
} as const;

describe("resolveTheme", () => {
  it("follows the OS when the preference is system", () => {
    expect(resolveTheme("system", "light")).toBe("light");
    expect(resolveTheme("system", "dark")).toBe("dark");
  });

  it("falls back to dark while the OS theme is unknown", () => {
    expect(resolveTheme("system", null)).toBe("dark");
  });

  it("lets an explicit choice win over the OS", () => {
    expect(resolveTheme("light", "dark")).toBe("light");
    expect(resolveTheme("dark", "light")).toBe("dark");
  });
});

describe("themeStore", () => {
  beforeEach(() => {
    themeStore.setState(initial);
  });

  it("starts on the OS theme with dark as the fallback", () => {
    const state = themeStore.getState();
    expect(state.preference).toBe("system");
    expect(state.resolved).toBe("dark");
  });

  it("resolves immediately when the preference is set", () => {
    themeStore.getState().setPreference("light");
    expect(themeStore.getState().resolved).toBe("light");
  });

  it("ignores later OS changes once the choice is explicit", () => {
    themeStore.getState().setPreference("dark");
    themeStore.getState().setSystemTheme("light");
    expect(themeStore.getState().resolved).toBe("dark");
  });

  it("follows the OS again when the preference returns to system", () => {
    themeStore.getState().setSystemTheme("light");
    themeStore.getState().setPreference("system");
    expect(themeStore.getState().resolved).toBe("light");
  });

  it("keeps the last resolved theme when the OS read fails", () => {
    themeStore.getState().setSystemTheme("light");
    themeStore.getState().setSystemTheme(null);
    expect(themeStore.getState().resolved).toBe("dark");
  });
});
