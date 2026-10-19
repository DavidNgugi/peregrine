import { describe, expect, it, vi } from "vitest";

// The helper names the plugin's command directly rather than going through its JavaScript
// facade, so the protocol is what this test pins: the command id and the argument name the
// Rust side reads. A plugin bump that renames either fails here rather than at a click.
const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({ invoke }));

import { copyText } from "./clipboard.js";

describe("copyText", () => {
  it("writes through the clipboard plugin's write-text command", async () => {
    invoke.mockResolvedValueOnce(undefined);

    await copyText("http://studio-mac.local:8642/");

    expect(invoke).toHaveBeenCalledWith("plugin:clipboard-manager|write_text", {
      text: "http://studio-mac.local:8642/",
    });
  });

  it("surfaces a refused write instead of pretending it copied", async () => {
    invoke.mockRejectedValueOnce(new Error("clipboard is not available"));

    await expect(copyText("x")).rejects.toThrow("clipboard is not available");
  });
});
