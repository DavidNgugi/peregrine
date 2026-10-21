import { readFileSync } from "node:fs";
import vm from "node:vm";
import { describe, expect, it } from "vitest";

// The shipped file is a classic script, so the test runs that exact source rather than a
// re-implementation with the same name.
const source = readFileSync(new URL("../server-url.js", import.meta.url), "utf8");
const context = { window: {}, URL };
vm.runInNewContext(source, context);
const normalize = context.window.PeregrineServerUrl.normalize;

describe("normalizeServerUrl", () => {
  it("accepts the address a person reads from the desktop, with or without a scheme", () => {
    expect(normalize("192.168.1.50:8642")).toBe("http://192.168.1.50:8642");
    expect(normalize(" http://192.168.1.50:8642/ ")).toBe("http://192.168.1.50:8642");
    expect(normalize("https://peregrine.example/lan/")).toBe("https://peregrine.example/lan/");
    expect(normalize("HTTP://JARVIS.LOCAL:8642")).toBe("http://jarvis.local:8642");
  });

  it("keeps a reverse-proxy path and drops a query or fragment", () => {
    expect(normalize("https://example.test/peregrine")).toBe("https://example.test/peregrine");
    expect(normalize("https://example.test/peregrine?x=1#top")).toBe(
      "https://example.test/peregrine",
    );
    expect(normalize("http://example.test/")).toBe("http://example.test");
  });

  it("refuses a scheme that is not http, http without a host, and credentials", () => {
    expect(normalize("ftp://192.168.1.50")).toBeNull();
    expect(normalize("file:///etc/passwd")).toBeNull();
    expect(normalize("javascript:alert(1)")).toBeNull();
    expect(normalize("http://")).toBeNull();
    expect(normalize("http://user:pass@192.168.1.50:8642")).toBeNull();
  });

  it("refuses empty, oversized, and whitespace-bearing input", () => {
    expect(normalize("")).toBeNull();
    expect(normalize("   ")).toBeNull();
    expect(normalize("192.168.1.50:8642/a b")).toBeNull();
    expect(normalize("http://" + "a".repeat(2048))).toBeNull();
    expect(normalize(null)).toBeNull();
    expect(normalize(7)).toBeNull();
  });
});
