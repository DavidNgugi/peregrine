/**
 * The typed call site for the generated IPC bindings.
 *
 * `commands.<name>()` answers with the generated `Result` union
 * (`{ status: "ok", data } | { status: "error", error }`), which is what a call
 * site that wants to branch on the failure should use directly. `unwrap` is the
 * same call with the failure turned into a thrown `IpcError`, so `try/catch` and
 * the shared error mapping in `errors.ts` see the shape they expect.
 *
 * The transport is `@tauri-apps/api`, which the host app supplies; nothing here
 * reaches the filesystem or the network on its own.
 */

import { commands } from "./generated/bindings.js";
import type { BuildInfo } from "./generated/bindings.js";
import { normalizeIpcError } from "./errors.js";

/** The generated result union for one command. */
export type CommandResult<T, E> = { status: "ok"; data: T } | { status: "error"; error: E };

/** True when a generated result union carries a payload rather than an error. */
export function isSuccessful<T, E>(
  result: CommandResult<T, E>,
): result is { status: "ok"; data: T } {
  return result.status === "ok";
}

/**
 * Awaits a generated result union and returns its payload, or throws.
 *
 * A call site that would rather `try/catch` than branch uses this, so the error it
 * catches is named only if the Rust enum declares it and optional fields are
 * absent, not null.
 */
export async function unwrap<T, E>(result: Promise<CommandResult<T, E>>): Promise<T> {
  const settled = await result;
  if (isSuccessful(settled)) {
    return settled.data;
  }
  throw normalizeIpcError(settled.error);
}

/** The running binary's build metadata, or a thrown structured IPC error. */
export async function getBuildInfo(): Promise<BuildInfo> {
  return unwrap(commands.buildInfo());
}
