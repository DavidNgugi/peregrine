import { describe, expect, it } from "vitest";
import {
  CATALOGUES,
  createI18n,
  DEFAULT_LOCALE,
  isSupportedLocale,
  SUPPORTED_LOCALES,
} from "./index.js";

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function leafPaths(value: unknown, prefix: string): string[] {
  if (!isRecord(value)) {
    return [prefix];
  }
  return Object.entries(value).flatMap(([key, child]) =>
    leafPaths(child, prefix === "" ? key : `${prefix}.${key}`),
  );
}

function leafValues(value: unknown): string[] {
  if (!isRecord(value)) {
    return typeof value === "string" ? [value] : [];
  }
  return Object.values(value).flatMap((child) => leafValues(child));
}

describe("catalogues", () => {
  it("ships a catalogue for every supported locale", () => {
    for (const locale of SUPPORTED_LOCALES) {
      expect(CATALOGUES[locale]).toBeDefined();
    }
  });

  it("has no empty, untrimmed, or non-string message", () => {
    for (const locale of SUPPORTED_LOCALES) {
      const values = leafValues(CATALOGUES[locale]);
      expect(values.length).toBeGreaterThan(0);
      for (const message of values) {
        expect(message).toBe(message.trim());
        expect(message.length).toBeGreaterThan(0);
      }
    }
  });

  it("keeps every locale's key set identical to the default locale", () => {
    const expected = new Set(leafPaths(CATALOGUES[DEFAULT_LOCALE], ""));
    for (const locale of SUPPORTED_LOCALES) {
      expect(new Set(leafPaths(CATALOGUES[locale], ""))).toEqual(expected);
    }
  });
});

describe("createI18n", () => {
  it("translates a known key and interpolates its parameters", () => {
    const i18n = createI18n();
    expect(i18n.t("shell.error.code", { code: "disk_full" })).toBe("Code disk_full");
    expect(i18n.t("discover.title")).toBe("Discover");
  });

  it("returns the key for a missing message so the gap is visible, never blank", () => {
    const i18n = createI18n();
    expect(i18n.t("discover.empty.missing")).toBe("discover.empty.missing");
  });
});

describe("isSupportedLocale", () => {
  it("accepts a shipped locale and rejects anything else", () => {
    expect(isSupportedLocale("en")).toBe(true);
    expect(isSupportedLocale("en-US")).toBe(false);
    expect(isSupportedLocale("EN")).toBe(false);
    expect(isSupportedLocale("")).toBe(false);
    expect(isSupportedLocale(undefined)).toBe(false);
    expect(isSupportedLocale(42)).toBe(false);
  });
});
