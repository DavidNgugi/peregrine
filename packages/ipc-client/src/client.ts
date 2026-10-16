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
  BrowsePageDto,
  BrowseSortDto,
  BuildInfo,
  GenreDto,
  HomeRailDto,
  IndexerStatusDto,
  RailKindDto,
  RecommendationsDto,
  JobStarted,
  LibraryRootDto,
  CaptureSessionDto,
  DownloadJobDto,
  ContinueWatchingDto,
  ProgressReport,
  EpisodeDto,
  SeasonDto,
  LibraryTitleDto,
  MatchAsDto,
  PlaybackSessionDto,
  ProwlarrStatusDto,
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
 * Asks the core to find and queue a release for a title.
 *
 * Answers the job, not the download: the search, the metadata fetch, and the safety gate all leave
 * the machine, so the caller watches the job rather than waiting for one (PG-E13).
 */
export async function acquire(titleId: number, minSeeders?: number): Promise<JobStarted> {
  return unwrap(commands.acquire({ id: titleId }, minSeeders ?? null));
}

/** The transfer queue, newest first. */
export async function listDownloads(): Promise<DownloadJobDto[]> {
  return unwrap(commands.listDownloads());
}

/** Pauses one transfer, answering the row as it stands after the move. */
export async function pauseDownload(id: number): Promise<DownloadJobDto> {
  return unwrap(commands.pauseDownload({ id }));
}

/** Resumes one paused transfer. */
export async function resumeDownload(id: number): Promise<DownloadJobDto> {
  return unwrap(commands.resumeDownload({ id }));
}

/** Removes one job from the queue. Files already written are a different operation. */
export async function removeDownload(id: number): Promise<DownloadJobDto> {
  return unwrap(commands.removeDownload({ id }));
}

/**
 * Whether an indexer is configured, and the base URL it was configured with.
 *
 * The API key is never part of the answer: it lives in the keychain and nothing reads it back.
 */
export async function indexerStatus(): Promise<IndexerStatusDto> {
  return unwrap(commands.indexerStatus());
}

/** Stores a Prowlarr base URL and API key, answering the status as it stands after the write. */
export async function setIndexer(baseUrl: string, apiKey: string): Promise<IndexerStatusDto> {
  return unwrap(commands.setIndexer(baseUrl, apiKey));
}

/** Asks a candidate Prowlarr configuration whether it answers. */
export async function testIndexer(baseUrl: string, apiKey: string): Promise<boolean> {
  return unwrap(commands.testIndexer(baseUrl, apiKey));
}

/** Removes the stored indexer configuration. */
export async function clearIndexer(): Promise<IndexerStatusDto> {
  return unwrap(commands.clearIndexer());
}

/** The managed Prowlarr pack's state: supported, installed, running, and its loopback URL. */
export async function prowlarrStatus(): Promise<ProwlarrStatusDto> {
  return unwrap(commands.prowlarrStatus());
}

/** Downloads and verifies the pinned Prowlarr pack; answers the job doing it. */
export async function installProwlarr(): Promise<JobStarted> {
  return unwrap(commands.installProwlarr());
}

/** Starts the installed Prowlarr; answers the job doing it. */
export async function startProwlarr(): Promise<JobStarted> {
  return unwrap(commands.startProwlarr());
}

/** Stops the managed Prowlarr; answers the state after the stop. */
export async function stopProwlarr(): Promise<ProwlarrStatusDto> {
  return unwrap(commands.stopProwlarr());
}

/** What the account is part-way through, most recently updated first. */
export async function continueWatching(): Promise<ContinueWatchingDto[]> {
  return unwrap(commands.continueWatching());
}

/** Records where the player is in a title or episode. */
export async function recordProgress(report: ProgressReport): Promise<void> {
  await unwrap(commands.recordProgress(report));
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
export async function searchProviders(
  query: string,
  limit?: number,
  kind?: RailKindDto,
): Promise<TitleMatchDto[]> {
  return unwrap(commands.searchProviders(query, limit ?? null, kind ?? null));
}

/**
 * Every rail the Discover screen draws, from what the catalogue and the cache already hold.
 *
 * A cache read and never a request: the core answers from the rails a job filled, and reports a
 * rail nobody has fetched yet as an empty rail rather than omitting it, which is what gives the
 * screen its layout before anything has painted (`FLOWS F2`). `poster` is an absolute path or
 * nothing, for the reason `getTitle`'s is.
 */
export async function homeRails(kind?: RailKindDto): Promise<HomeRailDto[]> {
  return unwrap(commands.homeRails(kind ?? null));
}

/**
 * Fills the rails the configured providers publish, artwork included, and answers with the job.
 *
 * A job rather than a wait, and the screen follows it through `job://state`. `coalesced` says
 * whether a fetch was already under way and this call joined it, so a screen can say so rather
 * than pretending it started one.
 */
export async function refreshRails(): Promise<JobStarted> {
  return unwrap(commands.refreshRails());
}

/**
 * One page of a filtered, ordered list from the configured providers.
 *
 * The second call here that leaves the machine, and it is one request per *page* rather than one per
 * filter change: a screen composes the request it wants and asks for it. What it resolves to is a
 * page of titles the catalogue owns — the core writes the provider's answer before answering — so an
 * item can be opened immediately and outlives the provider being reachable.
 *
 * `page` counts from one. `hasMore` is the provider's own answer rather than something a screen can
 * infer from `items.length`, which is what makes "load more" decidable.
 */
export async function browse(
  kind: RailKindDto,
  page: number,
  sort?: BrowseSortDto,
  genre?: number,
  minRating?: number,
): Promise<BrowsePageDto> {
  return unwrap(commands.browse(kind, page, sort ?? null, genre ?? null, minRating ?? null));
}

/**
 * The genres the configured providers filter by for one kind.
 *
 * Read once per session in the core, so a filter control opening for the second time costs a lock and
 * no request. `id` is the *provider's* own identifier and is what a browse request hands back.
 */
export async function genres(kind: RailKindDto): Promise<GenreDto[]> {
  return unwrap(commands.genres(kind));
}

/**
 * The titles the configured provider considers like one the catalogue holds.
 *
 * One request, and an empty answer is a real one rather than a failure: a title no configured
 * provider holds an identifier for has no recommendations to fetch, so a title's page draws no rail
 * for it rather than an error.
 */
export async function recommendations(titleId: number): Promise<RecommendationsDto> {
  return unwrap(commands.recommendations({ id: titleId }));
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
 * Starts playing one of a title's files, replacing whatever was playing.
 *
 * The one call whose answer is a URL. It leaves the machine nowhere: the address is this process's own
 * loopback listener, opened for one file and closed when the session ends (ADR 0023, SECURITY.md T10).
 * A refusal is a title that is not here, holds no copy on disk, holds a copy outside every configured
 * folder, or a file that cannot be served — each of them something the screen says beside the control.
 */
export async function startPlayback(titleId: number): Promise<PlaybackSessionDto> {
  return unwrap(commands.startPlayback({ id: titleId }));
}

/**
 * Opens a session for one title's file so a frame can be captured from it.
 *
 * Not `startPlayback` with a flag, and the name is the reason: nothing plays a capture session, and a
 * caller reading `startPlayback` here could not tell whether the film was meant to be playing. It holds
 * its own session, so a poster backfill cannot stop a film that is playing.
 */
export async function startPosterCapture(titleId: number): Promise<CaptureSessionDto> {
  return unwrap(commands.startPosterCapture({ id: titleId }));
}

/**
 * Stops a capture session, answering whether anything was open.
 */
export async function stopPosterCapture(sessionId: number): Promise<boolean> {
  return unwrap(commands.stopPosterCapture({ id: sessionId }));
}

/**
 * Stops whatever is playing, answering whether anything was.
 *
 * Deliberately not an error when nothing is playing: there is no state in which a caller asking for
 * silence should be told no.
 */
export async function stopPlayback(sessionId: number): Promise<boolean> {
  return unwrap(commands.stopPlayback({ id: sessionId }));
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
/**
 * One title's episodes, or one season of them when `season` is given.
 *
 * The season is a filter on one read rather than a second command, because the two questions differ by
 * one predicate — and a screen that had to pick between two reads would be a screen that could pick the
 * one that drifts.
 */
export async function listEpisodes(titleId: number, season?: number): Promise<EpisodeDto[]> {
  return unwrap(commands.listEpisodes({ id: titleId }, season ?? null, null));
}

/** One title's seasons, in the order its switcher lists them. */
export async function listSeasons(titleId: number): Promise<SeasonDto[]> {
  return unwrap(commands.listSeasons({ id: titleId }, null));
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
