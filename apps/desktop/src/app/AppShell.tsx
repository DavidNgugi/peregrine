import { IconButton, Sidebar, SidebarItem } from "@peregrine/ui";
import { ProfileAvatar, ProfileSwitcher, useProfiles } from "../features/profiles/index.js";
import { NotificationCentre } from "../features/notifications/components/NotificationCentre.js";
import type { ReactElement } from "react";
import { useCallback, useEffect, useId, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { BrowseScreen } from "../features/browse/index.js";
import { DiscoverScreen } from "../features/discover/index.js";
import { DownloadsScreen, useDownloads } from "../features/downloads/index.js";
import { HistoryScreen } from "../features/history/index.js";
import { JobsScreen, useJobsBridge } from "../features/jobs/index.js";
import { PlayerScreen } from "../features/player/index.js";
import {
  TitleScreen,
  useAcquire,
  useSeasons,
  useSimilarTitles,
  useTitle,
  useTitleFeed,
} from "../features/title/index.js";
import {
  LibraryScreen,
  useLibraryTitles,
  useMatchRules,
  useScan,
  useUnmatchedFiles,
  useLibraryView,
} from "../features/library/index.js";
import {
  SettingsScreen,
  useIndexers,
  useProwlarr,
  useLibraryRoots,
  useProviders,
  useShortcutDispatch,
  useShortcuts,
} from "../features/settings/index.js";
import { DownloadProgressProvider } from "../lib/download-progress.js";
import { PersonScreen } from "../features/person/components/PersonScreen.js";
import { useWatchlist } from "../features/watchlist/hooks/useWatchlist.js";
import { downloadMarks, downloadProgressMarks } from "../lib/download-status.js";
import { imageUrl } from "../lib/media.js";
import type { Platform } from "../lib/platform.js";
import { writePath } from "../lib/window-path.js";
import {
  back as goBackHistory,
  canGoBack,
  navigate as navigateTo,
  startHistory,
} from "./navigation.js";
import { ROUTE_FOR_ACTION, routePath, type Route, type TitleKind } from "./routes.js";
import { SCREENS } from "./screens.js";
import { useFullscreen } from "./useFullscreen.js";
import { useTheme } from "./useTheme.js";

interface AppShellProps {
  /** Which platform's window chrome to present; `null` when the core could not be asked. */
  readonly platform: Platform | null;
  /**
   * The route the window opened at.
   *
   * A prop rather than a read of `window.location`, so that the shell — the thing its tests render —
   * does not need a browser to exist. The entry that mounts it reads the path.
   */
  readonly route: Route;
}

/**
 * The chrome that persists across navigation: the window chrome, the sidebar, the current
 * screen, and the player bar (docs/ARCHITECTURE.md §6 rule 6, docs/DESIGN_SYSTEM.md §6).
 *
 * Only the content area is swapped when the screen changes. The chrome sits above the
 * sidebar and the player bar below it, both outside the element the screen renders into, so
 * a screen that mounts or unmounts cannot take the navigation or the player with it — which
 * is the property PG-E7-01's acceptance names, and the reason the screen switch is a
 * ternary inside one `main` rather than a component that replaces the whole tree.
 *
 * The screen switch is a route (`./routes.js`) rather than a name: the sidebar, the shortcut
 * dispatcher, and the path the window opened at all speak the same type, so a place in the
 * application is spelled once. What is still owed by PG-E7-02 is the other half of its acceptance —
 * cache-first loaders — and the routes for screens that do not exist yet; the old note about typed
 * routing living here is the part that has landed.
 * sidebar rows become links when it lands.
 */
export function AppShell({ platform, route: initialRoute }: AppShellProps): ReactElement {
  const { t } = useTranslation();
  const { resolved, setPreference } = useTheme();
  // Where the user is, which the sidebar, the shortcut dispatcher, and the screen switch all read
  // from one value. It starts where the window opened, and only this shell moves it — through the
  // history, so every screen except the first has somewhere to go back to (`./navigation.js`).
  const [history, setHistory] = useState(() => startHistory(initialRoute));
  const [profileOpen, setProfileOpen] = useState(false);
  const [profileBackdrops, setProfileBackdrops] = useState<readonly string[]>([]);
  const { active, add, book, rename, select } = useProfiles();
  const reportBackdrops = useCallback((urls: readonly string[]) => {
    setProfileBackdrops((current) =>
      current.length === urls.length && current.every((url, index) => url === urls[index])
        ? current
        : urls,
    );
  }, []);
  const route = history.current;
  const navigate = useCallback((next: Route) => {
    setHistory((current) => navigateTo(current, next));
  }, []);
  const goBack = useCallback(() => {
    setHistory(goBackHistory);
  }, []);
  const canBack = canGoBack(history);
  // Leaving the player is Back when there is a place behind it, and the title it was opened for
  // when the window opened straight onto the player — a reload keeps the path but not the stack.
  const playerId = route.kind === "player" ? route.id : null;
  const playerKind = route.kind === "player" ? (route.titleKind ?? "movie") : "movie";
  const leavePlayer = useCallback(() => {
    setHistory((current) => {
      if (canGoBack(current)) {
        return goBackHistory(current);
      }
      return playerId === null
        ? current
        : navigateTo(current, { kind: "title", titleKind: playerKind, id: playerId });
    });
  }, [playerId, playerKind]);

  // And the window's path follows it, so the two cannot drift: a reload — which a webview performs
  // for reasons it does not announce — returns the user to the screen they were on rather than to
  // the one the window happened to open at.
  useEffect(() => {
    writePath(routePath(route));
  }, [route]);

  // The icon and the name describe the same action — what the button would switch *to* —
  // so a sighted user and a screen reader are told the same thing.
  const nextTheme = resolved === "dark" ? "light" : "dark";
  const themeLabel = t(
    nextTheme === "light" ? "shell.theme.switchToLight" : "shell.theme.switchToDark",
  );

  // Whether the sidebar shows its labels, which is the user's own answer and not the window's width:
  // the width rule is the sidebar's. Nothing persists it yet — the store that would is PG-E1-02, and
  // the shortcut that would toggle it is PG-E21-05's registry entry.
  const [collapsed, setCollapsed] = useState(false);
  const fullscreen = useFullscreen();
  const sidebarId = useId();
  const collapseLabel = collapsed ? t("shell.nav.expand") : t("shell.nav.collapse");

  // **The window's top-left is the sidebar's**, because there is no title bar of our own (§8 rules a
  // fake one out). A macOS window in the `Overlay` style draws the traffic lights over this column, so
  // the strip leaves their band empty and puts the toggle *below* them — beside them there is no room
  // (the cluster is ~70 px wide and a collapsed rail is 60), and a control underneath them cannot be
  // clicked. That band is also the drag region: with no bar, this is where a person grabs the window.
  //
  // **In every other case the toggle sits level with the search field beside it**, which is a macOS
  // full screen (the lights hide until the pointer finds the top edge), Windows, and Linux. `22 px` is
  // the arithmetic and not a token: the hero's toolbar pads by 16, its field is 44 tall, so the
  // *marks* line up when a 20 px mark inside this 32 px button starts at 16 + (44 − 20) / 2 − (32 − 20)
  // / 2 = 22. Aligning the boxes instead would leave the two marks 6 px apart, which is the thing an
  // eye catches.
  //
  // **The toggle is a `SidebarItem` that is not a row**, and it is laid out as one so its mark lands
  // on the same x as the marks below it. A row is 32 px tall with a 20 px mark at its own 8 px padding
  // edge — 16 px from the column's edge — while the toggle centres a 20 px mark in a 32 px button, so
  // its box has to start at 10: `p-2` plus the 2 px below. A collapsed rail centres its rows instead
  // (their marks sit at 20), so `justify-center` takes over and the same box lands there.
  const sidebarHeader = (
    <div
      className={[
        "flex shrink-0 items-center p-2",
        platform === "macos" && !fullscreen ? "pt-[36px]" : "pt-[22px]",
        collapsed ? "justify-center" : "",
      ].join(" ")}
      data-tauri-drag-region={platform === "macos" ? "deep" : "false"}
    >
      <IconButton
        aria-controls={sidebarId}
        aria-expanded={!collapsed}
        className="ms-[2px]"
        icon="sidebar"
        label={collapseLabel}
        onClick={() => {
          setCollapsed((current) => !current);
        }}
        title={collapseLabel}
      />
      {collapsed ? null : (
        // The centre lives in the shell so a notification can reach the user from any screen; it
        // is hidden on the rail, where there is no room for a label.
        <div className="ms-auto">
          <NotificationCentre onNavigate={navigate} />
        </div>
      )}
    </div>
  );

  // Mounted by the shell rather than by the screen so a running job survives navigation.
  useJobsBridge();

  // The registry is the core's (PG-E1-08); the same answer drives the Settings list and
  // the keys this shell acts on, so the two cannot disagree.
  const shortcuts = useShortcuts();
  const libraries = useLibraryRoots();
  const providers = useProviders();
  const indexers = useIndexers();
  const prowlarr = useProwlarr();
  // The managed instance writes the Prowlarr key out of band when it starts, so the indexer
  // section's one read is stale the moment that happens. Re-read when the managed state flips to
  // running, so the section stops claiming nothing is configured.
  const managedProwlarrRunning = prowlarr.status?.running ?? false;
  const refreshIndexers = indexers.refresh;
  useEffect(() => {
    if (managedProwlarrRunning) {
      void refreshIndexers();
    }
  }, [managedProwlarrRunning, refreshIndexers]);
  // The key is written before the seed lands, so the flip to running alone is too early. While the
  // managed instance is up and the list is still empty, re-read for a bounded window so the seeded
  // public indexers appear without the user reopening the screen; the effect stops as soon as one
  // arrives.
  const configuredIndexers = indexers.indexers.length;
  useEffect(() => {
    if (!managedProwlarrRunning || configuredIndexers > 0) {
      return;
    }
    let attempts = 0;
    const timer = setInterval(() => {
      attempts += 1;
      void refreshIndexers();
      if (attempts >= 20) {
        clearInterval(timer);
      }
    }, 3000);
    return () => {
      clearInterval(timer);
    };
  }, [managedProwlarrRunning, configuredIndexers, refreshIndexers]);
  const library = useLibraryTitles();
  const downloads = useDownloads();
  const watchlist = useWatchlist();
  const watchlistedIds = useMemo(
    () => new Set(watchlist.rows.map((row) => row.titleId)),
    [watchlist.rows],
  );
  // One status per title, from the queue read the shell already holds. A screen hands a card the
  // mark for its own id rather than each subscribing to the queue: one read, one answer.
  const marks = useMemo(() => downloadMarks(downloads.downloads), [downloads.downloads]);
  const progressMarks = useMemo(
    () => downloadProgressMarks(downloads.downloads),
    [downloads.downloads],
  );
  // The poster wall behind the profile selector: the titles this device already holds, which is the
  // honest answer to "what is in this profile" while accounts do not exist.
  const profilePosters = useMemo(() => {
    const paths: string[] = [];
    for (const row of downloads.downloads) {
      if (row.poster !== null) paths.push(row.poster);
    }
    for (const title of library.titles) {
      if (title.poster !== null) paths.push(title.poster);
    }
    return [...new Set(paths)]
      .slice(0, 16)
      .map(imageUrl)
      .filter((url): url is string => url !== null);
  }, [downloads.downloads, library.titles]);
  const scan = useScan();
  const unmatched = useUnmatchedFiles();
  const rules = useMatchRules();
  // Which drawing the library screen uses. Held here rather than in the screen because `main` renders
  // one screen at a time, so a screen's own state does not survive a visit to a title and back.
  const libraryView = useLibraryView();
  // One title's screen is the only route with a parameter. The hook is always called — the rules of
  // hooks — and reads nothing when the route names no title.
  const acquire = useAcquire();
  const title = useTitle(route.kind === "title" ? route.id : null);
  // The recommendations the page draws under its episodes, asked for by the same rule and for the
  // same route: a provider read that the page can live without, which is why it is its own hook.
  const similar = useSimilarTitles(route.kind === "title" ? route.id : null);
  // **The feed the page's one list draws from**, built here with the other title reads so the screen
  // stays a projection: the recommendations are its head, and the genres they share are the queue of
  // pages behind them. One list and one scroll area, where the first shape was a rail with a shelf per
  // genre under it.
  const feed = useTitleFeed(
    route.kind === "title" ? route.titleKind : "movie",
    similar,
    route.kind === "title" ? route.id : null,
  );
  // The seasons and the season on screen, asked for by the same route. A film is answered without a
  // request, which is the same short circuit the title read makes.
  //
  // **The kind the *read* answered, once there is one.** The route's spelling is what the page can
  // say before its read lands, and it is not always right: the player's way back has no kind to go
  // on and spells "movie", and a download row may carry none. A season read driven by the route
  // would then never happen for a show — no request, no section, and nothing a reader could retry.
  const titleKind: TitleKind =
    title.title === null
      ? route.kind === "title"
        ? route.titleKind
        : "movie"
      : title.title.kind === "shows"
        ? "show"
        : "movie";
  // A finished acquisition writes the library rows the episode cards read, so the season read runs
  // again once the job settles. Without it the card that was just downloaded kept saying
  // "Not acquired" until the reader left the page and came back.
  const [acquiredRevision, setAcquiredRevision] = useState(0);
  // **The completion, not the request.** The acquire job settles when the download is queued; the
  // library rows only land when the transfer finishes. Watching the queue's completed rows is what
  // makes the episode that just finished stop saying "Not acquired" without leaving the page.
  const doneStates = new Set(["completed", "seeding"]);
  const completedForTitle =
    route.kind === "title"
      ? downloads.downloads.filter((job) => job.titleId === route.id && doneStates.has(job.state))
          .length
      : 0;
  const completedTotal = downloads.downloads.filter((job) => doneStates.has(job.state)).length;
  const seenCompletedTotal = useRef(completedTotal);
  useEffect(() => {
    if (completedTotal > seenCompletedTotal.current) {
      void library.refresh();
    }
    seenCompletedTotal.current = completedTotal;
  }, [completedTotal, library]);
  const seenCompleted = useRef<{ title: number | null; count: number }>({
    title: null,
    count: 0,
  });
  useEffect(() => {
    if (route.kind !== "title") {
      seenCompleted.current = { title: null, count: 0 };
      return;
    }
    if (seenCompleted.current.title !== route.id) {
      seenCompleted.current = { title: route.id, count: completedForTitle };
      return;
    }
    if (completedForTitle > seenCompleted.current.count) {
      setAcquiredRevision((current) => current + 1);
      void library.refresh();
    }
    seenCompleted.current.count = completedForTitle;
  }, [completedForTitle, route, library]);
  const acquiredTitle =
    acquire.outcome === "queued" && !acquire.running && !acquire.starting ? acquire.titleId : null;
  useEffect(() => {
    if (acquiredTitle !== null) {
      setAcquiredRevision((current) => current + 1);
    }
  }, [acquiredTitle]);
  // Coming back from the player is the moment the watched bar has something new to say: progress is
  // written while the player is open, and this screen was not re-reading behind it.
  const leavingPlayer = useRef(route.kind === "player");
  useEffect(() => {
    if (leavingPlayer.current && route.kind === "title") {
      setAcquiredRevision((current) => current + 1);
    }
    leavingPlayer.current = route.kind === "player";
  }, [route.kind]);
  const seasons = useSeasons(
    route.kind === "title" ? route.id : null,
    titleKind,
    title.refreshes + title.seasonUpdates + acquiredRevision,
  );
  const openScreen = useCallback(
    (action: string) => {
      const target = ROUTE_FOR_ACTION[action];
      if (target !== undefined) {
        navigate(target);
      }
    },
    [navigate],
  );
  useShortcutDispatch(shortcuts.bindings, openScreen);

  return (
    <div className="flex h-full flex-col bg-surface-0 text-text-primary">
      <div className="flex min-h-0 flex-1">
        <Sidebar
          collapsed={collapsed}
          footer={
            <div className={collapsed ? "flex justify-center" : "flex items-center gap-2"}>
              <ProfileAvatar
                label={t("profile.switch")}
                onClick={() => {
                  setProfileOpen(true);
                }}
                profile={active}
              />
              {collapsed ? null : (
                <span className="truncate text-sm text-text-secondary">{active.name}</span>
              )}
            </div>
          }
          header={sidebarHeader}
          id={sidebarId}
          label={t("shell.nav.label")}
        >
          {SCREENS.map((entry) => (
            <SidebarItem
              current={entry.route.kind === route.kind}
              icon={entry.icon}
              key={entry.route.kind}
              label={t(entry.labelKey)}
              onSelect={() => {
                navigate(entry.route);
              }}
            />
          ))}
        </Sidebar>
        {/* `deep` makes the whole column draggable except the controls inside it — Tauri's drag
            handler walks the event's path and skips anything clickable — which is what gives a window
            with no title bar somewhere to be grabbed besides the sidebar's strip. */}
        {/* No drag region here, deliberately: the chrome's is the sidebar's strip, and a screen's own
            area carrying one would let a screen take the window's behaviour with it — which is the
            property `App.test.tsx` pins. On macOS the overlay title bar drags on its own; on Windows
            and Linux the frame and its caption do. */}
        <main className="min-h-0 min-w-0 flex-1 overflow-hidden">
          <DownloadProgressProvider value={progressMarks}>
            {route.kind === "discover" ? (
              /* A bare `/` is All, which is the default the toolbar starts on and what `parseRoute`
               answers when the URL carries no `type`. */
              <DiscoverScreen
                downloads={marks}
                watchlist={watchlist.rows}
                libraryTitles={library.titles}
                onBackdrops={reportBackdrops}
                onNavigate={navigate}
                titleKind={route.titleKind}
                trailing={
                  <>
                    {/* The theme control, which used to live in the title bar. Same shape as the toolbar's
                      own icon buttons — 44 px, on the translucent surface — because it sits beside them:
                      the refresh control that follows it is the neighbour it has to match. */}
                    <IconButton
                      className="size-11 rounded-full border border-border-subtle bg-surface-0/70"
                      icon={nextTheme === "light" ? "sun" : "moon"}
                      label={themeLabel}
                      onClick={() => {
                        setPreference(nextTheme);
                      }}
                      title={themeLabel}
                      variant="ghost"
                    />
                    <ProfileAvatar
                      label={t("profile.switch")}
                      onClick={() => {
                        setProfileOpen(true);
                      }}
                      profile={active}
                      shape="circle"
                      size="toolbar"
                    />
                  </>
                }
              />
            ) : null}
            {route.kind === "browse" ? (
              /* The kind is the route's, so the toolbar's switch is a navigation and the window's
               path says which category is on screen. A route naming none is the default kind — see
               `BrowseScreen`. */
              <BrowseScreen
                backLabel={t("shell.back")}
                downloadRows={downloads.downloads}
                downloads={marks}
                onBack={canBack ? goBack : undefined}
                onNavigate={navigate}
                titleKind={route.titleKind}
                watchlisted={watchlistedIds}
              />
            ) : null}
            {route.kind === "library" ? (
              <LibraryScreen
                backLabel={t("shell.back")}
                downloads={marks}
                onBack={canBack ? goBack : undefined}
                onNavigate={navigate}
                onView={libraryView.setView}
                rules={rules}
                scan={scan}
                titles={library}
                unmatched={unmatched}
                watchlisted={watchlistedIds}
                view={libraryView.view}
              />
            ) : null}
            {route.kind === "person" ? (
              <PersonScreen
                backLabel={t("shell.back")}
                id={route.id}
                onBack={canBack ? goBack : undefined}
                onNavigate={navigate}
                watchlisted={watchlistedIds}
              />
            ) : null}
            {route.kind === "downloads" ? (
              <DownloadsScreen
                backLabel={t("shell.back")}
                downloads={downloads}
                onBack={canBack ? goBack : undefined}
                onNavigate={navigate}
              />
            ) : null}
            {route.kind === "history" ? <HistoryScreen onNavigate={navigate} /> : null}
            {route.kind === "diagnostics" ? (
              <JobsScreen backLabel={t("shell.back")} onBack={canBack ? goBack : undefined} />
            ) : null}
            {route.kind === "settings" ? (
              <SettingsScreen
                backLabel={t("shell.back")}
                indexers={indexers}
                prowlarr={prowlarr}
                libraries={libraries}
                onBack={canBack ? goBack : undefined}
                providers={providers}
                shortcuts={shortcuts}
              />
            ) : null}
            {route.kind === "player" ? (
              /* The player is a screen of its own rather than a state inside the title page: the video
               wants the content area, the title page's counts and seasons want to stay where they are,
               and a route is what makes Back mean what it says. The session it opens lives exactly as
               long as this component does (ADR 0023, SECURITY.md T10). */
              <PlayerScreen
                episode={route.episode}
                id={route.id}
                season={route.season}
                /* Back rather than a route of the shell's own: the player was opened from a title,
                 so the place behind it is that title, and going back is what keeps the history
                 rather than pushing a second copy of the same page. */
                onClose={leavePlayer}
              />
            ) : null}
            {route.kind === "title" ? (
              <TitleScreen
                detail={title}
                onWatchlist={() => {
                  watchlist.toggle(route.id);
                }}
                watchlisted={watchlist.has(route.id)}
                feed={feed}
                kind={route.titleKind}
                /* **Keyed by which title it is, and its absence was a bug a reader hit.** Opening a
                 title from another title's feed is the same component in the same position, so React
                 reconciled it onto the *same* `<section>` — and that section is the screen's scroller.
                 The new page therefore opened at the old page's scroll offset, so following a
                 recommendation looked like nothing had happened: the reader stayed part-way down a
                 different title and had to scroll up to find its name. Changing the key remounts the
                 screen, which resets the scroll and, just as importantly, drops the previous title's
                 component state — the season switcher would otherwise open on the season the *last*
                 title was left on. */
                key={`${route.titleKind}:${String(route.id)}`}
                acquire={acquire}
                backLabel={t("shell.back")}
                downloadRows={downloads.downloads}
                downloads={marks}
                onBack={canBack ? goBack : undefined}
                onNavigate={navigate}
                seasons={seasons}
                watchlistIds={watchlistedIds}
              />
            ) : null}
          </DownloadProgressProvider>
        </main>
      </div>
      <ProfileSwitcher
        activeId={book.activeId}
        backdrops={profileBackdrops}
        onAdd={add}
        onClose={() => {
          setProfileOpen(false);
        }}
        onRename={rename}
        onSelect={select}
        open={profileOpen}
        posters={profilePosters}
        profiles={book.profiles}
      />
    </div>
  );
}
