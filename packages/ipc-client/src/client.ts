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
import type {
  BuildInfo,
  JobStarted,
  LibraryRootDto,
  LibraryTitleDto,
  RootKind,
  RootRef,
  RootScanMode,
  ShortcutBinding,
} from "./generated/bindings.js";
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

/**
 * Every shortcut the core's registry declares, already rendered for this platform.
 *
 * The Settings list draws this and nothing else: a list maintained beside the registry
 * would be a second source for the same fact (PG-E1-08).
 */
export async function listShortcuts(): Promise<ShortcutBinding[]> {
  return unwrap(commands.listShortcuts());
}

/**
 * The library roots the user has configured, newest first.
 *
 * `limit` is the page size rather than a cursor: the settings screen shows one page, and the
 * generated contract has no large-integer argument for a cursor to travel in — the same reason
 * `jobs::list` takes a limit and nothing else.
 */
export async function listLibraryRoots(limit?: number): Promise<LibraryRootDto[]> {
  return unwrap(commands.listRoots(limit ?? null));
}

/**
 * Adds a folder as a library root.
 *
 * `path` is what the shell's folder picker returned, which is text the picker has not validated
 * (`UntrustedPath`, PG-E1-12); the core is what decides whether it can be a root, and its refusal
 * arrives as an `IpcError` this module throws.
 */
export async function addLibraryRoot(
  path: string,
  kind: RootKind,
  scanMode: RootScanMode,
): Promise<LibraryRootDto> {
  return unwrap(commands.addRoot(path, kind, scanMode));
}

/**
 * A page of the titles the library holds, newest first.
 *
 * `limit` is the page size rather than a cursor, for the reason `listLibraryRoots` gives: the
 * generated contract has no large-integer argument for a cursor to travel in.
 */
export async function listLibraryTitles(limit?: number): Promise<LibraryTitleDto[]> {
  return unwrap(commands.listTitles(limit ?? null));
}

/**
 * Starts a scan of every library root and answers with the job to watch.
 *
 * A scan asked for while one is running throws a `Conflict` rather than queueing: the walk is the
 * expensive part, and a queue of identical walks is not a feature. The caller shows the refusal as
 * it shows any other.
 */
export async function scanLibrary(): Promise<JobStarted> {
  return unwrap(commands.scanNow());
}

/**
 * Removes a library root, answering whether the row was there.
 *
 * Removing a root stops it being scanned and removes nothing else: the files stay, and so do the
 * items and the watch progress (PG-E14-01, PG-E14-13). There is nothing here to confirm.
 */
export async function removeLibraryRoot(id: number): Promise<boolean> {
  const root: RootRef = { id };
  return unwrap(commands.removeRoot(root));
}

/** Whether Peregrine is registered to start with the operating system's session. */
export async function getAutoStartEnabled(): Promise<boolean> {
  return unwrap(commands.autostartEnabled());
}

/** Registers or removes the login item. */
export async function setAutoStart(enabled: boolean): Promise<void> {
  await unwrap(commands.setAutostart(enabled));
}
