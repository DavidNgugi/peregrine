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
  EpisodeDto,
  LibraryTitleDto,
  MatchAsDto,
  MatchRuleDto,
  TitleDetailDto,
  TitleMatchDto,
  UnmatchedFileDto,
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
 * A page of titles that match the user's own text.
 *
 * An empty query answers nothing rather than everything: it is not a search, and a picker that listed
 * the whole catalogue when the field was empty would be the library screen again.
 */
export async function searchTitles(query: string, limit?: number): Promise<TitleMatchDto[]> {
  return unwrap(commands.searchTitles(query, limit ?? null));
}

/**
 * Searches the configured metadata providers, then reads what the catalogue holds.
 *
 * The one call here that leaves the machine. It is a single request per search rather than one per
 * keystroke for that reason, and what it resolves to is a title the *catalogue* owns — the core writes
 * the provider's answer before answering (`PG-E6-04`) — so a result can be opened and outlives the
 * provider being reachable.
 */
export async function searchProviders(query: string, limit?: number): Promise<TitleMatchDto[]> {
  return unwrap(commands.searchProviders(query, limit ?? null));
}

/**
 * One title as its own screen reads it: the library's answer, plus where its poster is.
 *
 * `null` covers both "this account may not see it" and "no title has that id": the two read the same
 * to a person, and the screen says the title is not here either way (I37). `poster` is an absolute
 * path or nothing — turning it into something an `<img>` can load is `lib/media.ts`'s job, and the
 * bytes never travel over IPC (AGENTS R11). `refreshDue` is the core's rule for whether opening the
 * screen should ask a provider for the title, and a screen must not second-guess it (AGENTS R7).
 */
export async function getTitle(id: number): Promise<TitleDetailDto | null> {
  return unwrap(commands.getTitle({ id }));
}

/**
 * Starts filling one title from its provider, artwork included, and answers with the job to watch.
 *
 * A job rather than a wait, because it leaves the machine twice — once for the title's details and
 * once for its poster — and a screen that blocked on a provider having a slow day would be a screen
 * that froze (ARCHITECTURE §7 rule 2). Keyed by the title in the core, so opening the same screen
 * twice waits on one refresh rather than spending two requests on one answer.
 */
export async function refreshTitle(id: number, kind: RootKind): Promise<JobStarted> {
  return unwrap(commands.refreshTitle({ id }, kind));
}

/** A title's episodes, in the order a person watches them. */
export async function listEpisodes(titleId: number): Promise<EpisodeDto[]> {
  return unwrap(commands.listEpisodes({ id: titleId }, null));
}

/**
 * A page of the rules a user's corrections became, newest first.
 *
 * A rule places every later name that reads the way the one it was made from did, which is why the
 * screen shows them: an answer a user gave once keeps acting, so it has to be visible and
 * withdrawable.
 */
export async function listMatchRules(limit?: number): Promise<MatchRuleDto[]> {
  return unwrap(commands.listMatchRules(limit ?? null));
}

/** Removes one rule, reporting whether it was there. */
export async function forgetMatchRule(id: number): Promise<boolean> {
  return unwrap(commands.forgetMatchRule({ id }));
}

/**
 * A page of the files a scan could not place, newest first.
 *
 * The queue a person answers: each row carries what the parse read before it stopped, which is the
 * only thing that can tell them which value to trust.
 */
export async function listUnmatchedFiles(limit?: number): Promise<UnmatchedFileDto[]> {
  return unwrap(commands.listUnmatched(limit ?? null));
}

/**
 * Answers one parked file.
 *
 * `asTitle` says which title the answer files it under: what its name read, or the title the scan
 * offered — which joins a title the library already holds rather than creating a near-duplicate of
 * it. Answers whether it placed anything: a file a scan reached while the row waited is already
 * indexed, and clearing the stale row is the whole of that answer rather than a failure.
 */
export async function matchUnmatchedFile(
  id: number,
  asTitle: MatchAsDto = "parsed",
  titleId?: number,
): Promise<boolean> {
  return unwrap(commands.matchUnmatched({ id, asTitle, titleId: titleId ?? null }));
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
