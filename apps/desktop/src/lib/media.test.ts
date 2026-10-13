import { beforeEach, describe, expect, it, vi } from "vitest";

// The webview asks Tauri's own bridge to build the URL, so the test replaces that module
// and drives the call site in Node. The factory is hoisted above the `import` below, which
// is why the spies live in `vi.hoisted` rather than beside the test.
const bridge = vi.hoisted(() => ({
  convertFileSrc: vi.fn((path: string) => `asset://localhost/${encodeURIComponent(path)}`),
  inTauri: true,
}));

vi.mock("@tauri-apps/api/core", () => ({
  convertFileSrc: bridge.convertFileSrc,
  isTauri: () => bridge.inTauri,
}));

const { imageUrl, isImagePath } = await import("./media.js");

beforeEach(() => {
  bridge.convertFileSrc.mockClear();
  bridge.inTauri = true;
});

describe("imageUrl", () => {
  it("turns an absolute cache reference into an asset URL without touching IPC", () => {
    const url = imageUrl("/Users/someone/Library/Caches/app.peregrine.desktop/images/poster.webp");

    expect(url).toBe(
      "asset://localhost/%2FUsers%2Fsomeone%2FLibrary%2FCaches%2Fapp.peregrine.desktop%2Fimages%2Fposter.webp",
    );
    expect(bridge.convertFileSrc).toHaveBeenCalledTimes(1);
  });

  it("accepts a Windows drive-absolute reference", () => {
    imageUrl("C:\\Users\\someone\\AppData\\Local\\peregrine\\images\\poster.webp");

    expect(bridge.convertFileSrc).toHaveBeenCalledWith(
      "C:\\Users\\someone\\AppData\\Local\\peregrine\\images\\poster.webp",
    );
  });

  it("accepts the verbatim form Windows canonicalization produces", () => {
    // `std::fs::canonicalize` answers `\\?\C:\…` on Windows, and that is the path the shell hands
    // the runtime, so this is the *ordinary* case on that platform rather than an exotic one. A check
    // that refused it would render a placeholder for every poster ever cached, and nothing would
    // report it: the placeholder is what a missing poster looks like too.
    const verbatim = "\\\\?\\C:\\Users\\someone\\AppData\\Local\\peregrine\\images\\poster.webp";

    expect(imageUrl(verbatim)).toBe(`asset://localhost/${encodeURIComponent(verbatim)}`);
    // Passed through as it arrived: the asset scope canonicalizes what it is asked about, so both
    // forms name the same file.
    expect(bridge.convertFileSrc).toHaveBeenCalledWith(verbatim);
  });

  it("accepts the verbatim form of a UNC path", () => {
    const unc = "\\\\?\\UNC\\server\\share\\peregrine\\images\\poster.webp";

    expect(imageUrl(unc)).not.toBeNull();
  });

  it("still refuses a traversal written behind a verbatim prefix", () => {
    // The prefix is stripped for the shape check only, so it cannot be used to smuggle a `..` past it.
    expect(imageUrl("\\\\?\\C:\\cache\\..\\..\\etc\\passwd")).toBeNull();
  });

  it("renders nothing outside the Tauri shell rather than inventing a URL", () => {
    bridge.inTauri = false;

    expect(imageUrl("/cache/poster.webp")).toBeNull();
    expect(bridge.convertFileSrc).not.toHaveBeenCalled();
  });

  it("refuses a base64 payload: bytes must not travel through a DTO", () => {
    expect(imageUrl("data:image/webp;base64,UklGRg==")).toBeNull();
    expect(bridge.convertFileSrc).not.toHaveBeenCalled();
  });

  it("refuses a remote URL, which the strict CSP would not load anyway", () => {
    expect(imageUrl("https://image.tmdb.org/t/p/w342/poster.jpg")).toBeNull();
    expect(bridge.convertFileSrc).not.toHaveBeenCalled();
  });

  it("refuses a relative path, which would resolve against the webview's document", () => {
    expect(imageUrl("images/poster.webp")).toBeNull();
    expect(imageUrl("./poster.webp")).toBeNull();
  });

  it("refuses a traversal out of the cache", () => {
    expect(imageUrl("/cache/images/../../../etc/passwd")).toBeNull();
    expect(imageUrl("C:\\cache\\..\\secret.wav")).toBeNull();
  });

  it("refuses a path carrying a NUL or a control character", () => {
    expect(imageUrl("/cache/poster.webp\u0000.png")).toBeNull();
    expect(imageUrl("/cache/poster\n.webp")).toBeNull();
  });

  it("refuses everything that is not a non-empty string", () => {
    for (const value of [null, undefined, 7, {}, [], true, ""]) {
      expect(imageUrl(value)).toBeNull();
    }
    expect(bridge.convertFileSrc).not.toHaveBeenCalled();
  });
});

describe("isImagePath", () => {
  it("narrows an absolute path and nothing else", () => {
    expect(isImagePath("/cache/images/thumb.webp")).toBe(true);
    expect(isImagePath("C:\\cache\\thumb.webp")).toBe(true);
    expect(isImagePath("cache/thumb.webp")).toBe(false);
    expect(isImagePath("asset://localhost/cache/thumb.webp")).toBe(false);
  });
});
