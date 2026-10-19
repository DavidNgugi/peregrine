import { invoke } from "@tauri-apps/api/core";

/**
 * Writes one string to the system clipboard.
 *
 * The plugin's JavaScript facade would be a package dependency for a single call, and this is
 * exactly the command it makes: the Rust plugin is the implementation and `write-text` is the
 * capability the ACL grants. Reading the clipboard is neither exposed here nor granted there.
 */
export async function copyText(text: string): Promise<void> {
  await invoke("plugin:clipboard-manager|write_text", { text });
}
