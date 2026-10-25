import { afterEach, describe, expect, it, vi } from "vitest";

import { readViewState, writeViewState } from "./view-state.js";

class MemoryStorage {
  private readonly values = new Map<string, string>();

  get length(): number {
    return this.values.size;
  }

  clear(): void {
    this.values.clear();
  }

  getItem(key: string): string | null {
    return this.values.get(key) ?? null;
  }

  key(index: number): string | null {
    return [...this.values.keys()][index] ?? null;
  }

  removeItem(key: string): void {
    this.values.delete(key);
  }

  setItem(key: string, value: string): void {
    this.values.set(key, value);
  }
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("view state", () => {
  it("round-trips a filter through the session store", () => {
    vi.stubGlobal("window", { sessionStorage: new MemoryStorage() });
    writeViewState("library.query", "alien");
    expect(readViewState("library.query", "")).toBe("alien");
  });

  it("falls back when the store is empty or holds junk", () => {
    vi.stubGlobal("window", { sessionStorage: new MemoryStorage() });
    expect(readViewState("missing", "fallback")).toBe("fallback");
    window.sessionStorage.setItem("peregrine.view.junk", "{not json}");
    expect(readViewState("junk", "fallback")).toBe("fallback");
  });
});
