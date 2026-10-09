/**
 * The typed call site for the generated IPC bindings.
 *
 * `commands.<name>()` answers with the generated `Result` union
 * (`{ status: "ok", data } | { status: "error", error }`), which is what a call
 * site that wants to branch on the failure should use directly. `getBuildInfo` is
 * the same call with the failure turned into a thrown `IpcError`, so `try/catch`
 * and the shared error mapping in `errors.ts` see the shape they expect.
 *
 * The transport is `@tauri-apps/api`, which the host app supplies; nothing here
 * reaches the filesystem or the network on its own.
 */

import { commands } from "./generated/bindings.js";
import type { BuildInfo } from "./generated/bindings.js";
import { normalizeIpcError } from "./errors.js";

/** True when a generated result union carries a payload rather than an error. */
export function isSuccessful<T, E>(
  result: { status: "ok"; data: T } | { status: "error"; error: E },
): result is { status: "ok"; data: T } {
  return result.status === "ok";
}

/** The running binary's build metadata, or a thrown structured IPC error. */
export async function getBuildInfo(): Promise<BuildInfo> {
  const result = await commands.buildInfo();
  if (isSuccessful(result)) {
    return result.data;
  }
  // Reuses the boundary normalizer, so an error the UI receives here is named
  // only if the Rust enum declares it and optional fields are absent, not null.
  throw normalizeIpcError(result.error);
}
