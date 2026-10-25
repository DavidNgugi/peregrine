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
  AttributionDto,
  BackupDto,
  BrowsePageDto,
  RestoreDto,
  BrowseSortDto,
  BuildInfo,
  GenreDto,
  HomeRailDto,
  IndexerStatusDto,
  AddIndexerRequest,
  ConfiguredIndexerDto,
  IndexerDefinitionDto,
  IndexerRef,
  IndexerSummaryDto,
  RailItemDto,
  RailKindDto,
  RecommendationsDto,
  JobStarted,
  LibraryRootDto,
  CaptureSessionDto,
  DeletionPlanDto,
  MediaDeletionPlanDto,
  DownloadJobDto,
  ReleaseOfferDto,
  NotificationDto,
  NotificationPreferencesDto,
  ContinueWatchingDto,
  WatchDayDto,
  WatchHistoryDto,
  ScheduledDownloadDto,
  WatchlistEntryDto,
  ProgressReport,
  EpisodeDto,
  SeasonDto,
  LibraryStorageDto,
  LibraryTitleDto,
  MatchAsDto,
  PlaybackSessionDto,
  ProfileDto,
  ProfileRef,
  ProwlarrStatusDto,
  MatchRuleDto,
  TitleDetailDto,
  WatchAvailabilityDto,
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
 * What this build is obliged to say about the providers it can talk to (PG-E5-19).
 *
 * A read with no runtime behind it: the notices are fixed strings, so an installation with no key
 * and no started runtime can still show them.
 */
export async function getProviderAttribution(): Promise<AttributionDto[]> {
  return unwrap(commands.providerAttribution());
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
 * Asks the core to find and queue a release for a title, or for one episode of it.
 *
 * Answers the job, not the download: the search, the metadata fetch, and the safety gate all leave
 * the machine, so the caller watches the job rather than waiting for one (PG-E13). `season` and
 * `episode` travel together or not at all, which the command itself enforces.
 */
export async function acquire(
  titleId: number,
  minSeeders?: number,
  season?: number,
  episode?: number,
): Promise<JobStarted> {
  return unwrap(
    commands.acquire({ id: titleId }, minSeeders ?? null, season ?? null, episode ?? null),
  );
}

/**
 * Asks the core to find and queue a whole season of a title.
 *
 * The job searches once and acquires the better-seeded side — the season pack or a single episode
 * of that season — so the caller names the season and nothing else (PG-E12-16).
 */
export async function acquireSeason(
  titleId: number,
  season: number,
  minSeeders?: number,
): Promise<JobStarted> {
  return unwrap(commands.acquireSeason({ id: titleId }, season, minSeeders ?? null));
}

/** The transfer queue, newest first. */
export async function listDownloads(): Promise<DownloadJobDto[]> {
  return unwrap(commands.listDownloads());
}

/** Schedules a title or one episode for the first date it may be fetched. */
export async function scheduleDownload(
  titleId: number,
  season: number | null,
  episode: number | null,
  releaseOn: string,
): Promise<void> {
  await unwrap(commands.scheduleDownload({ id: titleId }, season, episode, releaseOn));
}

/** Every schedule the account holds, soonest first. */
export async function downloadSchedules(): Promise<ScheduledDownloadDto[]> {
  return unwrap(commands.downloadSchedules());
}

/** Cancels one schedule; a job it already started is cancelled from the queue. */
export async function cancelDownloadSchedule(id: number): Promise<void> {
  await unwrap(commands.cancelDownloadSchedule({ id }));
}

/** The quality downloads prefer, as its wire spelling, or null when the stored list names none. */
export async function getPreferredQuality(): Promise<string | null> {
  return unwrap(commands.preferredQuality());
}

/** Records the quality downloads should prefer. */
export async function setPreferredQuality(quality: string): Promise<void> {
  await unwrap(commands.setPreferredQuality(quality));
}

/** Pauses one transfer, answering the row as it stands after the move. */
export async function pauseDownload(id: number): Promise<DownloadJobDto> {
  return unwrap(commands.pauseDownload({ id }));
}

/** Resumes one paused transfer. */
export async function resumeDownload(id: number): Promise<DownloadJobDto> {
  return unwrap(commands.resumeDownload({ id }));
}

/** Asks a torrent's swarm for peers again, answering the row as it stands after the move. */
export async function findDownloadPeers(id: number): Promise<DownloadJobDto> {
  return unwrap(commands.findDownloadPeers({ id }));
}

/** Removes one job from the queue. Files already written are a different operation. */
export async function removeDownload(id: number): Promise<DownloadJobDto> {
  return unwrap(commands.removeDownload({ id }));
}

/** Clears one finished queue entry. The bytes on disk stay; only the record goes. */
export async function dismissDownload(id: number): Promise<boolean> {
  return unwrap(commands.dismissDownload({ id }));
}

/** Clears one failed acquisition preflight. It never admitted a download or wrote a file. */
export async function dismissPreflight(id: number): Promise<boolean> {
  return unwrap(commands.dismissPreflight({ id }));
}

/** Enumerates what deleting one download would remove, without removing anything. */
export async function planDownloadDeletion(id: number): Promise<DeletionPlanDto> {
  return unwrap(commands.planDownloadDeletion({ id }));
}

/** Moves one download's files to the operating system's trash and clears its queue entry. */
export async function deleteDownload(id: number): Promise<DeletionPlanDto> {
  return unwrap(commands.deleteDownload({ id }));
}

/** Clears every finished queue entry, answering how many went. */
export async function clearFinishedDownloads(): Promise<number> {
  return unwrap(commands.clearFinishedDownloads());
}

/**
 * The releases a search left at qualities the preferred profile excluded.
 *
 * The core keeps them when nothing matches the preference, ordered best-first. Their sources stay
 * server-side, so this answers ids and metadata and nothing that could be fetched from here (R11).
 */
export async function listReleaseOffers(titleId: number): Promise<ReleaseOfferDto[]> {
  return unwrap(commands.listReleaseOffers({ id: titleId }));
}

/** The user's notification consent and the catalogue defaults around it. */
export async function notificationPreferences(): Promise<NotificationPreferencesDto> {
  return unwrap(commands.notificationPreferences());
}

/**
 * Records the user's answer to the notification consent question.
 *
 * The screen asks the operating system first when the answer is `true`, so the value it sends is
 * the effective decision rather than the intent (R31: the user is asked, never defaulted).
 */
export async function setNotificationConsent(
  granted: boolean,
): Promise<NotificationPreferencesDto> {
  return unwrap(commands.setNotificationConsent(granted));
}

/** The recent notification centre rows, newest first (R31 makes the row the record). */
export async function listNotifications(limit = 20): Promise<NotificationDto[]> {
  return unwrap(commands.listNotifications(limit));
}

/** Marks one centre row read, answering whether it changed. */
export async function markNotificationRead(id: number): Promise<boolean> {
  return unwrap(commands.markNotificationRead({ id }));
}

/** Marks every unread centre row read, answering how many changed. */
export async function markAllNotificationsRead(): Promise<number> {
  return unwrap(commands.markAllNotificationsRead());
}

/** Dismisses one centre row, so it leaves the list. */
export async function dismissNotification(id: number): Promise<boolean> {
  return unwrap(commands.dismissNotification({ id }));
}

/** Queues the release the user picked, without running a second search. */
export async function chooseReleaseOffer(
  offerId: number,
  minSeeders?: number,
): Promise<JobStarted> {
  return unwrap(commands.chooseReleaseOffer({ id: offerId }, minSeeders ?? null));
}

/** Dismisses one offer so a later search does not surface it again. */
export async function dismissReleaseOffer(offerId: number): Promise<boolean> {
  return unwrap(commands.dismissReleaseOffer({ id: offerId }));
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

/** Lists the indexers already configured in Prowlarr. */
export async function listIndexers(): Promise<ConfiguredIndexerDto[]> {
  return unwrap(commands.listIndexers());
}

/** Lists the addable definitions, without their fields. */
export async function indexerCatalog(): Promise<IndexerSummaryDto[]> {
  return unwrap(commands.indexerCatalog());
}

/** One definition's fields, read when the user picks it. */
export async function indexerDefinition(key: string): Promise<IndexerDefinitionDto | null> {
  return unwrap(commands.indexerDefinition(key));
}

/** Adds one indexer and answers the refreshed list. */
export async function addIndexer(request: AddIndexerRequest): Promise<ConfiguredIndexerDto[]> {
  return unwrap(commands.addIndexer(request));
}

/** Removes one indexer and answers the refreshed list. */
export async function removeIndexer(indexer: IndexerRef): Promise<ConfiguredIndexerDto[]> {
  return unwrap(commands.removeIndexer(indexer));
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

/** The account's viewing history, newest first. */
export async function viewingHistory(limit = 200): Promise<WatchHistoryDto[]> {
  return unwrap(commands.viewingHistory(limit));
}

export async function watchMetrics(from?: number, to?: number): Promise<WatchDayDto[]> {
  return unwrap(commands.watchMetrics(from ?? null, to ?? null));
}

/** Removes progress for one title, or all of it when no title is named. */
export async function clearWatchHistory(titleId?: number): Promise<number> {
  return unwrap(commands.clearWatchHistory(titleId === undefined ? null : { id: titleId }));
}

/** The account's watchlist, newest first. */
export async function watchlist(): Promise<WatchlistEntryDto[]> {
  return unwrap(commands.watchlist());
}

/** Adds one title to the watchlist. */
export async function addToWatchlist(titleId: number): Promise<void> {
  await unwrap(commands.addToWatchlist({ id: titleId }));
}

/** Removes one title from the watchlist, answering whether it was on it. */
export async function removeFromWatchlist(titleId: number): Promise<boolean> {
  return unwrap(commands.removeFromWatchlist({ id: titleId }));
}

/** Records where the player is in a title or episode. */
export async function recordProgress(report: ProgressReport): Promise<void> {
  await unwrap(commands.recordProgress(report));
}

/** Finalizes a playback that reached the media end. */
export async function finalizeProgress(report: ProgressReport): Promise<void> {
  await unwrap(commands.finalizeProgress(report));
}

/** Marks one episode watched or unwatched, on the user's authority. */
export async function setEpisodeWatched(
  titleId: number,
  season: number,
  episode: number,
  watched: boolean,
): Promise<void> {
  await unwrap(commands.setEpisodeWatched({ id: titleId }, season, episode, watched));
}

/** Marks every episode of one season watched or unwatched, answering how many it touched. */
export async function setSeasonWatched(
  titleId: number,
  season: number,
  watched: boolean,
): Promise<number> {
  return unwrap(commands.setSeasonWatched({ id: titleId }, season, watched));
}

/** Marks a whole title watched or unwatched, for a film rather than for one episode. */
export async function setTitleWatched(titleId: number, watched: boolean): Promise<void> {
  await unwrap(commands.setTitleWatched({ id: titleId }, watched));
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

/** The visible library's stored media bytes and file count. */
export async function libraryStorage(): Promise<LibraryStorageDto> {
  return unwrap(commands.libraryStorage());
}

/** Every household profile, with the active one marked. */
export async function listProfiles(): Promise<ProfileDto[]> {
  return unwrap(commands.listProfiles());
}

/** Creates a profile and makes it active. */
export async function createProfile(
  name: string,
  kind: string,
  avatarRef: string | null,
): Promise<ProfileDto> {
  return unwrap(commands.createProfile(name, kind, avatarRef));
}

/** Makes one profile active. */
export async function selectProfile(profile: ProfileRef): Promise<ProfileDto> {
  return unwrap(commands.selectProfile(profile));
}

/** Writes one profile's name and avatar. */
export async function updateProfile(
  profile: ProfileRef,
  name: string,
  avatarRef: string | null,
): Promise<ProfileDto> {
  return unwrap(commands.updateProfile(profile, name, avatarRef));
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
 * Searches the configured providers and answers with the tiles a poster grid draws.
 *
 * **The tile-shaped sibling of `searchProviders`.** A result carries the year, the score, the
 * synopsis, and the cached poster path, because it went through the same list writer a browse page
 * does — so a search result and a page tile cannot describe one title two ways. The provider's answer
 * is written into the catalogue before it is returned, so a result outlives the provider being
 * reachable. `poster` is an absolute path or nothing, for the reason `browse`'s is.
 */
export async function searchTiles(
  query: string,
  limit?: number,
  kind?: RailKindDto,
): Promise<RailItemDto[]> {
  return unwrap(commands.searchTiles(query, limit ?? null, kind ?? null));
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

export async function planMediaDeletion(
  titleId: number,
  seasonNumber?: number,
  episodeNumber?: number,
): Promise<MediaDeletionPlanDto> {
  return unwrap(
    commands.planMediaDeletion({
      titleId,
      seasonNumber: seasonNumber ?? null,
      episodeNumber: episodeNumber ?? null,
    }),
  );
}

export async function deleteMedia(
  titleId: number,
  seasonNumber?: number,
  episodeNumber?: number,
): Promise<MediaDeletionPlanDto> {
  return unwrap(
    commands.deleteMedia({
      titleId,
      seasonNumber: seasonNumber ?? null,
      episodeNumber: episodeNumber ?? null,
    }),
  );
}

/**
 * Where a title can be watched in one region, as the provider last answered.
 *
 * A cache read: fetching is the title refresh the screen already starts when `refreshDue` says so.
 * `fetchedAt` is what lets the section tell "nobody has asked" from "nothing is streaming it here".
 */
export async function watchAvailability(
  titleId: number,
  region: string,
): Promise<WatchAvailabilityDto> {
  return unwrap(commands.watchAvailability({ id: titleId }, region));
}

/**
 * Starts playing one of a title's files, replacing whatever was playing.
 *
 * The one call whose answer is a URL. It leaves the machine nowhere: the address is this process's own
 * loopback listener, opened for one file and closed when the session ends (ADR 0023, SECURITY.md T10).
 * A refusal is a title that is not here, holds no copy on disk, holds a copy outside every configured
 * folder, or a file that cannot be served — each of them something the screen says beside the control.
 */
export async function startPlayback(
  titleId: number,
  season: number | null = null,
  episode: number | null = null,
): Promise<PlaybackSessionDto> {
  return unwrap(commands.startPlayback({ id: titleId }, season, episode));
}

/**
 * Opens one title's file in mpv, for a container the page's media element cannot demux.
 *
 * The bytes never cross IPC: mpv opens the file itself, so this command carries only the ids. The
 * season and episode travel together or not at all; an episode play must hand mpv that episode's
 * file rather than the first file the title owns. A refusal is mpv missing, or one of the same file
 * refusals `startPlayback` reports.
 */
export async function publicIp(): Promise<string> {
  return unwrap(commands.publicIp());
}

export async function playExternally(
  titleId: number,
  season?: number,
  episode?: number,
): Promise<void> {
  await unwrap(commands.playExternally({ id: titleId }, season ?? null, episode ?? null));
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

/**
 * Writes a backup of this installation into the chosen folder.
 *
 * The folder is an untrusted picker result until the core canonicalizes it, and the file name is
 * generated by the core so no part of the user's text becomes a path component.
 */
export async function createBackup(directory: string): Promise<BackupDto> {
  return unwrap(commands.createBackup(directory));
}

/**
 * Validates a backup and stages it for the next launch, which the core starts.
 *
 * The call may not settle: the app is restarted once the file is staged, and a rejected promise
 * is the only outcome a caller can observe. That is why the screen treats a pending restore as a
 * success it cannot see, rather than waiting for an answer that may never arrive.
 */
export async function restoreBackup(source: string): Promise<RestoreDto> {
  return unwrap(commands.restoreBackup(source));
}
