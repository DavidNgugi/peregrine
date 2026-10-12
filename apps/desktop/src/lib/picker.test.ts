import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mergesFileAndFolderPickers, parsePlatform } from "./platform.js";

// The webview calls the plugin through Tauri's own bridge, so the test replaces the
// plugin module and drives the picker's call site in Node. `isTauri` is read from
// `globalThis` by `@tauri-apps/api/core`, so it is stubbed on `globalThis` instead of
// mocked: that is exactly the check the real code makes.
const open = vi.fn();
vi.mock("@tauri-apps/plugin-dialog", () => ({ open }));

const { isUntrustedPath, pickFile, pickFolder } = await import("./picker.js");

const isTauriKey = "isTauri";
const globals = globalThis as Record<string, unknown>;

function inTauri(): void {
  globals[isTauriKey] = (): boolean => true;
}

beforeEach(() => {
  open.mockReset();
  delete globals[isTauriKey];
});

afterEach(() => {
  delete globals[isTauriKey];
});

describe("pickFile", () => {
  it("returns a single picked path as a typed result, never a bare string", async () => {
    inTauri();
    open.mockResolvedValueOnce("/Users/someone/Downloads/movie.mkv");

    const result = await pickFile();

    expect(result).toEqual({ kind: "files", paths: ["/Users/someone/Downloads/movie.mkv"] });
    const path = result?.kind === "files" ? result.paths[0] : undefined;
    expect(isUntrustedPath(path)).toBe(true);
  });

  it("normalizes a multiple selection into one shape", async () => {
    inTauri();
    open.mockResolvedValueOnce(["/a.mkv", "/b.mkv"]);

    await expect(pickFile({ multiple: true })).resolves.toEqual({
      kind: "files",
      paths: ["/a.mkv", "/b.mkv"],
    });
    expect(open).toHaveBeenCalledWith(
      expect.objectContaining({ multiple: true, directory: false }),
    );
  });

  it("reports a cancellation as its own result rather than an error", async () => {
    inTauri();
    open.mockResolvedValueOnce(null);

    await expect(pickFile()).resolves.toEqual({ kind: "cancelled" });
  });

  it("treats an empty selection as a cancellation", async () => {
    inTauri();
    open.mockResolvedValueOnce([]);

    await expect(pickFile({ multiple: true })).resolves.toEqual({ kind: "cancelled" });
  });

  it("drops an entry the platform reported as something other than a path", async () => {
    inTauri();
    // A hostile or broken bridge can answer with anything; only a non-empty string is a
    // path, and this is the boundary where that is decided.
    open.mockResolvedValueOnce(["/a.mkv", "", 7, null]);

    await expect(pickFile({ multiple: true })).resolves.toEqual({
      kind: "files",
      paths: ["/a.mkv"],
    });
  });

  it("passes the title, start directory, and filters through to the plugin", async () => {
    inTauri();
    open.mockResolvedValueOnce(null);

    await pickFile({
      title: "Choose a video",
      startIn: "/Users/someone/Movies",
      filters: [{ name: "Video", extensions: ["mkv", "mp4"] }],
    });

    expect(open).toHaveBeenCalledWith({
      directory: false,
      multiple: false,
      title: "Choose a video",
      defaultPath: "/Users/someone/Movies",
      filters: [{ name: "Video", extensions: ["mkv", "mp4"] }],
    });
  });

  it("opens no dialog at all outside the Tauri shell", async () => {
    await expect(pickFile()).resolves.toBeNull();
    expect(open).not.toHaveBeenCalled();
  });
});

describe("pickFolder", () => {
  it("asks for a directory and returns a typed result", async () => {
    inTauri();
    open.mockResolvedValueOnce("/Users/someone/Media");

    await expect(pickFolder()).resolves.toEqual({
      kind: "folder",
      path: "/Users/someone/Media",
    });
    expect(open).toHaveBeenCalledWith(
      expect.objectContaining({ directory: true, multiple: false, recursive: false }),
    );
  });

  it("reports a cancellation", async () => {
    inTauri();
    open.mockResolvedValueOnce(null);

    await expect(pickFolder()).resolves.toEqual({ kind: "cancelled" });
  });

  it("does not accept a multi-selection, which a folder dialog never makes", async () => {
    inTauri();
    // Defensive: the plugin's type says this cannot happen for `multiple: false`, and if
    // that ever changes, reporting a cancellation beats treating an array as a path.
    open.mockResolvedValueOnce(["/a", "/b"] as unknown as string);

    await expect(pickFolder()).resolves.toEqual({ kind: "cancelled" });
  });

  it("opens no dialog at all outside the Tauri shell", async () => {
    await expect(pickFolder()).resolves.toBeNull();
    expect(open).not.toHaveBeenCalled();
  });
});

describe("isUntrustedPath", () => {
  it("accepts a non-empty string and nothing else", () => {
    expect(isUntrustedPath("/tmp/x")).toBe(true);
    expect(isUntrustedPath("")).toBe(false);
    expect(isUntrustedPath(7)).toBe(false);
    expect(isUntrustedPath(null)).toBe(false);
    expect(isUntrustedPath(undefined)).toBe(false);
  });
});

describe("mergesFileAndFolderPickers", () => {
  it("is false only on Windows, which shows a different dialog for a folder", () => {
    expect(mergesFileAndFolderPickers("windows")).toBe(false);
    expect(mergesFileAndFolderPickers("macos")).toBe(true);
    expect(mergesFileAndFolderPickers("linux")).toBe(true);
    // An unread platform is not Windows: assuming the two-dialog behaviour would make the
    // picker ask for the wrong thing, and assuming one dialog only mislabels a control.
    expect(mergesFileAndFolderPickers(null)).toBe(true);
  });

  it("agrees with the platform parse the chrome already uses", () => {
    expect(mergesFileAndFolderPickers(parsePlatform("windows-x86_64"))).toBe(false);
    expect(mergesFileAndFolderPickers(parsePlatform("macos-aarch64"))).toBe(true);
  });
});
