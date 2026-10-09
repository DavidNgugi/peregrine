import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  BRAND_HUE_TOKEN,
  isThemeName,
  LIGHT_THEME_OVERRIDES,
  SEMANTIC_COLOUR_TOKENS,
  TYPOGRAPHY_FONT_TOKENS,
  TYPOGRAPHY_SIZE_TOKENS,
} from "./index.js";

// The stylesheet is the only place the values live, so the test reads it: a token
// renamed on one side fails here instead of shipping a component with no colour.
const stylesheet = readFileSync(new URL("./tokens.css", import.meta.url), "utf8");

function blockContents(selector: string): string {
  const start = stylesheet.indexOf(`${selector} {`);
  const end = start === -1 ? -1 : stylesheet.indexOf("}", start);
  return start === -1 || end === -1 ? "" : stylesheet.slice(start, end);
}

function declaredTokens(source: string): string[] {
  return [...source.matchAll(/(--[a-z0-9-]+)\s*:/gu)].map((match) => match[1] ?? "");
}

function declarations(source: string): Map<string, string> {
  const found = new Map<string, string>();
  for (const match of source.matchAll(/(--[a-z0-9-]+)\s*:\s*([^;]+);/gu)) {
    const name = match[1];
    const value = match[2];
    if (name !== undefined && value !== undefined) {
      found.set(name, value.trim());
    }
  }
  return found;
}

const root = blockContents(":root");
const light = blockContents('[data-theme="light"]');
const rootTokens = declaredTokens(root);
const lightTokens = declaredTokens(light);

describe("tokens.css", () => {
  it("declares the brand hue and every semantic colour in the base theme", () => {
    expect(rootTokens).toContain(BRAND_HUE_TOKEN);
    for (const token of SEMANTIC_COLOUR_TOKENS) {
      expect(rootTokens).toContain(token);
    }
  });

  it("declares every typography token in the base theme", () => {
    for (const token of [...TYPOGRAPHY_FONT_TOKENS, ...TYPOGRAPHY_SIZE_TOKENS]) {
      expect(rootTokens).toContain(token);
    }
  });

  it("gives every text token a line-height companion", () => {
    for (const token of TYPOGRAPHY_SIZE_TOKENS) {
      expect(rootTokens).toContain(`${token}--line-height`);
    }
  });

  it("overrides only tokens the base theme also declares", () => {
    for (const token of lightTokens) {
      expect(rootTokens).toContain(token);
    }
    for (const token of LIGHT_THEME_OVERRIDES) {
      expect(lightTokens).toContain(token);
    }
  });

  it("declares no token twice in one block", () => {
    expect(new Set(rootTokens).size).toBe(rootTokens.length);
    expect(new Set(lightTokens).size).toBe(lightTokens.length);
  });

  it("authors every colour in OKLCH or as another token", () => {
    for (const source of [root, light]) {
      const values = declarations(source);
      for (const token of SEMANTIC_COLOUR_TOKENS) {
        const value = values.get(token);
        if (value !== undefined) {
          expect(value).toMatch(/^(oklch\(|var\()/u);
        }
      }
    }
  });
});

describe("isThemeName", () => {
  it("accepts the shipped themes and rejects anything else", () => {
    expect(isThemeName("dark")).toBe(true);
    expect(isThemeName("light")).toBe(true);
    expect(isThemeName("Dark")).toBe(false);
    expect(isThemeName("system")).toBe(false);
    expect(isThemeName("")).toBe(false);
    expect(isThemeName(null)).toBe(false);
    expect(isThemeName(7)).toBe(false);
  });
});
