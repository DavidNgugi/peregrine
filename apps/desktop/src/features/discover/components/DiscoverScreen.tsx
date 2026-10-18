import {
  Button,
  EmptyState,
  ErrorState,
  IconButton,
  Link,
  Rail,
  Screen,
  TextInput,
  TitleCard,
} from "@peregrine/ui";
import type {
  LibraryTitleDto,
  RailItemDto,
  RailKindDto,
  TitleMatchDto,
  WatchlistEntryDto,
} from "@peregrine/ipc-client";
import { useEffect, type ReactElement, type ReactNode } from "react";
import { useTranslation } from "react-i18next";

import { routePath, type Route } from "../../../app/routes.js";
import { downloadCardProps, type DownloadMarks } from "../../../lib/download-status.js";
import { useDownloadProgress } from "../../../lib/download-progress.js";
import { imageUrl } from "../../../lib/media.js";
import { MIN_QUERY_CHARS, useTitleSearch, type SearchSource } from "../../../lib/title-search.js";
import { useContinueWatching, type ContinueWatchingView } from "../hooks/useContinueWatching.js";
import { useHomeRails, type HomeRailsView } from "../hooks/useHomeRails.js";
import { DiscoverHero, DiscoverHeroSkeleton, type HeroSlide } from "./DiscoverHero.js";
import { heroSlides } from "./hero-rotation.js";

/** Nothing downloading, for the view drawn from synthetic props in a test. */
const NO_DOWNLOADS: DownloadMarks = new Map();

/**
 * Discover: the search box, a hero, and the rails the providers publish.
 *
 * **There is no spinner and there is no loading body** (`DESIGN_SYSTEM §11` rule 2). The core
 * answers the rails cache-first (`FLOWS F2`), so every rail has its heading and its own empty state
 * from the first frame — a rail nobody has fetched yet is an *empty rail*, not an absent one, which
 * is what keeps the headings below it from moving when the fetch lands. What the screen says while
 * it waits is nothing; what it says when there is nothing is the empty state, and its copy names
 * the thing to do about it — except when the core says a provider is configured and the first fetch
 * is in the air, where the section says so in one line instead of telling the user to set up the key
 * they already set up.
 *
 * **No business rule lives here** (`AGENTS R7`): which titles are in which rail, whether artwork was
 * fetched, whether a title is a film or a series, whether the rails are empty because nothing can
 * fill them or because the fetch is still running, and whether there is a file to play all arrive as
 * answers. The screen picks the hero — the first rail that has items, which is `heroSlides`'s rule
 * and the core's own rule for whose backdrops are worth fetching — because "which rectangle is
 * biggest" is layout rather than policy, and it maps a cached path to a URL through `lib/media.ts`,
 * the one place that names the asset protocol.
 *
 * Search is submitted rather than fired per keystroke because it leaves the machine (AGENTS R24).
 *
 * **The two halves are split so the drawing can be tested.** [`DiscoverScreen`] is the data layer —
 * two hooks and one piece of local state — and [`DiscoverView`] takes what they answered as props,
 * which is what lets a Node test render the rails, the hero, and the empty states from synthetic
 * DTOs with no bridge, no store, and no network.
 */

/** The rail kinds, as translation keys. A rail the core names and this build has no key for falls
 * back to one heading rather than an empty one, so a newer core cannot render a nameless rail. */
const RAIL_HEADINGS: Readonly<Record<string, string>> = {
  trending_week: "discover.rail.trendingWeek",
  popular_movies: "discover.rail.popularMovies",
  top_rated_movies: "discover.rail.topRatedMovies",
  upcoming_movies: "discover.rail.upcomingMovies",
  popular_shows: "discover.rail.popularShows",
  on_the_air_shows: "discover.rail.onTheAirShows",
  top_rated_shows: "discover.rail.topRatedShows",
};

/** One line a tile and the hero both use: "1999 · Film", or the kind alone when the year is unknown. */
export function itemMeta(item: RailItemDto, kind: string): string {
  return item.year === null ? kind : `${String(item.year)} · ${kind}`;
}

/** What the drawing needs, which is what the hooks answered plus what the search box holds. */
export interface DiscoverViewProps {
  readonly rails: HomeRailsView;
  /**
   * What the account is part-way through, drawn as its own rail above the provider rails.
   *
   * Optional so a caller that draws the view from synthetic props — the rendering tests — need not
   * invent rows it is not testing; an absent rail is an empty one.
   */
  readonly continueWatching?: ContinueWatchingView;
  readonly search: {
    readonly results: readonly TitleMatchDto[];
    readonly searching: boolean;
    readonly failed: boolean;
    readonly searched: boolean;
    /**
     * Which of the two answered — the catalogue's own index, or a provider asked because the catalogue
     * had nothing. The screen says which, because the two are not the same claim: one is instant and
     * offline, and the other has just written what it found into the catalogue.
     */
    readonly source: SearchSource;
    /** Runs the search now rather than after the debounce: the form's submit, and the magnifier. */
    readonly searchNow: () => void;
    readonly query: string;
    readonly setQuery: (query: string) => void;
  };
  /**
   * What the hero's Play does, when this build can play anything at all.
   *
   * Omitted by the only caller there is, deliberately: `PG-E15` owns playback and no surface for it
   * exists yet, so the hero draws Play disabled with the reason the core gave rather than an enabled
   * control that fires nothing (`PG-E1-07`).
   */
  readonly onPlay?: (slide: HeroSlide) => void;
  readonly onNavigate: (route: Route) => void;
  /**
   * The featured title's backdrop, when the hero has one.
   *
   * The profile selector draws the same picture at full window, so it reads the hero's answer
   * rather than fetching a second catalogue for decoration.
   */
  readonly onBackdrops?: ((urls: readonly string[]) => void) | undefined;
  /** A download status per title, so a rail's tiles wear the mark of a transfer in flight. */
  readonly downloads?: DownloadMarks | undefined;
  /** The account's saved titles, drawn as the first rail. */
  readonly watchlist?: readonly WatchlistEntryDto[] | undefined;
  /** The account's own scanned titles, drawn as their own rail. */
  readonly libraryTitles?: readonly LibraryTitleDto[] | undefined;
  /**
   * The selected type, or `undefined` for **All**.
   *
   * `| undefined` is not the usual "this prop may be omitted": here the absence *is* a choice — the
   * unfiltered read — so a caller passing a value that happens to be `undefined` means All rather
   * than nothing, and `exactOptionalPropertyTypes` would otherwise force every call site to spell
   * that with a conditional spread.
   */
  readonly titleKind?: RailKindDto | undefined;
  readonly onKindChange?: (kind: RailKindDto | undefined) => void;
  /**
   * One more control for the toolbar's trailing end — the shell's theme switch, which has no other
   * home now that the window draws no title bar.
   */
  readonly trailing?: ReactNode;
}

/** Search and category controls shared by the hero and its empty-artwork fallback. */
export function DiscoverToolbar({
  search,
  rails,
  titleKind,
  onKindChange,
  feedback,
  trailing,
}: {
  readonly search: DiscoverViewProps["search"];
  readonly rails: Pick<HomeRailsView, "refresh" | "refreshing">;
  readonly titleKind: RailKindDto | undefined;
  readonly onKindChange: (kind: RailKindDto | undefined) => void;
  readonly feedback?: ReactNode;
  readonly trailing?: ReactNode;
}): ReactElement {
  const { t } = useTranslation();
  return (
    <div className="relative flex flex-wrap items-center gap-3" data-discover-toolbar="">
      <form
        role="search"
        // **The pill is the field, so the pill draws the focus ring.** The input inside it is restyled
        // into a transparent, borderless, rounded box (`[&_input]:...` below), and with the primitive's
        // default focus ring it drew a *second* rounded outline inside this one — two concentric pills
        // on focus, which is what a reader saw. `focus-within` puts the token ring on the shape a person
        // reads as the field, and `focusRing="none"` is how the input is told the caller owns it.
        className="flex min-w-0 basis-full items-center gap-2 rounded-full border border-border-strong bg-surface-0/70 px-3 focus-within:outline-2 focus-within:outline-offset-2 focus-within:outline-focus-ring sm:basis-auto sm:flex-1"
        onSubmit={(event) => {
          event.preventDefault();
          search.searchNow();
        }}
      >
        <TextInput
          autoComplete="off"
          className="min-w-0 flex-1 [&_input]:h-11 [&_input]:rounded-full [&_input]:border-0 [&_input]:bg-transparent"
          focusRing="none"
          label={t("discover.search.label")}
          labelHidden
          name="query"
          type="search"
          onChange={(event) => search.setQuery(event.target.value)}
          placeholder={t("discover.search.placeholder")}
          value={search.query}
        />
        {/* **The magnifier rather than the word.** The pill is the field, and the control that
            submits it is the field's own affordance — a word inside a text field reads as part of the
            label, which is why the reference puts the mark there. It is still a submit button in a
            form, so `Enter` in the field and a click do the same thing — which is also the escape hatch
            from the debounce: a reader who has decided the catalogue does not have it should not wait
            for a timer. Its accessible name is the same string the word was (`IconButton` takes a
            required `label`, and `title` shows it to a pointer). `size-9` keeps a 36 px target inside
            the 44 px pill — the mark is 20 px (§9's toolbar size) with 8 px around it. */}
        <IconButton
          className="size-9"
          disabled={search.searching || search.query.trim().length < MIN_QUERY_CHARS}
          icon="search"
          label={t("discover.search.action")}
          title={t("discover.search.action")}
          type="submit"
          variant="ghost"
        />
      </form>
      <div className="ms-auto flex shrink-0 items-center gap-2">
        <div
          role="group"
          aria-label={t("discover.filter.label")}
          className="flex items-center gap-1 rounded-full border border-border-subtle bg-surface-0/70 p-1"
        >
          {/* **All is a choice and not the absence of one.** Every rail is what a home screen shows by
            default, and a first launch that drew only films would look empty to a household whose
            provider publishes mostly series — so the unfiltered read is the one the control starts
            on, and `undefined` is how it travels: the core's `home_rails(None)` keeps every rail and
            `search_providers` asks for both kinds. */}
          {([undefined, "movie", "show"] as const).map((kind) => (
            <Button
              key={kind ?? "all"}
              aria-pressed={titleKind === kind}
              className="rounded-full"
              onClick={() => onKindChange(kind)}
              variant={titleKind === kind ? "secondary" : "ghost"}
            >
              {t(
                kind === undefined
                  ? "discover.filter.all"
                  : kind === "movie"
                    ? "discover.filter.movies"
                    : "discover.filter.shows",
              )}
            </Button>
          ))}
        </div>
        {/* **An icon, at the row's own height, on the trailing edge.** As a word it was a smaller box
            than everything beside it — `h-7` against the search field's `h-11` and the filter group's
            `p-1` plus `h-9` — so it read as a control that had drifted out of the row it belongs to.
            `size-11` puts it on the same 44 px as both, and the trailing edge is where an action that
            re-asks every provider belongs: it is not a filter, and sitting between the search field and
            the selector made it look like one.

            It turns while a fetch is in flight, which is the one piece of state an icon-only control
            can show — `Button`'s text used to say "Refreshing…" and a dimmed glyph says nothing. That
            is information rather than decoration, which is what §5 rule 4 permits (the same argument
            the loading sweep records), it is `transform` only (§5 rule 2), and under reduced motion it
            stops while the state stays in the label, `disabled`, and the native tooltip. */}
        <IconButton
          className={[
            // A surface and a border of its own, because a lone icon on the hero artwork is a glyph
            // floating beside two pills rather than a control: the filter group carries the same
            // `border-border-subtle` and the same translucent surface, so the two read as siblings.
            "size-11 rounded-full border border-border-subtle bg-surface-0/70",
            rails.refreshing ? "[&>svg]:animate-spin [&>svg]:motion-reduce:animate-none" : "",
          ].join(" ")}
          disabled={rails.refreshing}
          icon="refresh"
          label={rails.refreshing ? t("discover.refresh.running") : t("discover.refresh.action")}
          onClick={() => {
            void rails.refresh();
          }}
          title={rails.refreshing ? t("discover.refresh.running") : t("discover.refresh.action")}
          variant="ghost"
        />
        {trailing}
      </div>
      {feedback}
    </div>
  );
}

/** The screen as it is drawn, from answers. */
export function DiscoverView({
  rails,
  continueWatching,
  watchlist,
  libraryTitles,
  search,
  onPlay,
  onNavigate,
  onBackdrops,
  downloads,
  titleKind,
  onKindChange,
  trailing,
}: DiscoverViewProps): ReactElement {
  const { t } = useTranslation();
  const marks = downloads ?? NO_DOWNLOADS;
  const saved = watchlist ?? [];
  const watchlisted = new Set(saved.map((row) => row.titleId));
  // Newest arrival first, because this rail answers "what did I just add?" rather than "what do I
  // own?". `addedAt` is the newest file's arrival, so a download that filled an older title's last
  // season moves that title to the front.
  const scanned = [...(libraryTitles ?? [])].sort((left, right) => right.addedAt - left.addedAt);
  const progress = useDownloadProgress();

  const routeFor = (item: RailItemDto): Route => ({
    id: item.titleId,
    kind: "title",
    titleKind: item.kind,
  });
  const routeForResult = (row: TitleMatchDto): Route => ({
    id: row.id,
    kind: "title",
    titleKind: row.kind === "movies" ? "movie" : "show",
  });
  const kindFor = (item: RailItemDto): string =>
    item.kind === "movie" ? t("discover.kind.movie") : t("discover.kind.show");
  const kindForResult = (row: TitleMatchDto): string =>
    row.kind === "movies" ? t("discover.kind.movie") : t("discover.kind.show");
  // The score the tile draws and announces. `null` is the core's "the provider said nothing", which
  // draws and announces nothing rather than a zero nobody claimed.
  //
  // **No count anywhere, and the core still carries it** — a list answer sends how many people voted,
  // `RailItem` holds it, and `RailItemDto` hands it over, which is where it stays until a screen that
  // is *about* one title has a use for it. On a rail of posters it was noise on every tile, and on the
  // hero's band it was the least useful thing on the line.
  const ratingLabel = (item: RailItemDto): string | null =>
    item.rating === null ? null : t("card.rating", { score: item.rating });

  // The hero's rotation, resolved here because a URL, a year and a count are the caller's words and
  // not the kit's: the component draws what it is handed and decides only what the timing is.
  const slides: readonly HeroSlide[] = heroSlides(rails.rails).map((item) => ({
    id: item.titleId,
    name: item.name,
    kind: item.kind,
    meta: itemMeta(item, kindFor(item)),
    href: routePath(routeFor(item)),
    backdrop: imageUrl(item.backdrop),
    poster: imageUrl(item.poster),
    overview: item.overview,
    score: item.rating === null ? null : item.rating.toFixed(1),
    scoreLabel: ratingLabel(item),
    playable: item.playable,
  }));

  // The selector paints the hero's backdrop at full window. Reported from here because this screen
  // already holds the answer, and a second catalogue read for decoration would be a second query.
  const featuredBackdrops = [
    ...new Set(slides.map((slide) => slide.backdrop).filter((url): url is string => url !== null)),
  ];
  const backdropKey = featuredBackdrops.join("|");
  useEffect(() => {
    onBackdrops?.(backdropKey.length === 0 ? [] : backdropKey.split("|"));
  }, [backdropKey, onBackdrops]);

  // The Discover kind filter applies to the two rails this screen owns as well as to the provider
  // ones: a film must not appear under TV shows, and a series must not appear under Films.
  // undefined is All, which admits both.
  const watching = (continueWatching?.rows ?? []).filter(
    (row) => titleKind === undefined || row.kind === titleKind,
  );
  const nothingAtAll = rails.rails.every((rail) => rail.items.length === 0);
  // The one state the screen says out loud rather than drawing as an empty catalogue: a provider is
  // configured and the fetch is in the air, so the answer is "coming", not "nothing" (`FLOWS F2`).
  const firstFetch = rails.state === "fetching";
  // A read of a *different* selection — the Movies / TV shows switch — is also a moment where what is
  // on screen is not the answer yet, and the hook answers with no rails until it is. That is one IPC
  // round trip, far too short to draw placeholders in, and long enough that "Nothing here yet" would
  // be a claim about a catalogue nobody has read: so the screen says nothing rather than something
  // wrong, and the fetch that *is* worth placeholders is the one above.
  const unknownYet = firstFetch || rails.loading;

  const feedback =
    search.searching || search.failed || search.searched ? (
      <div className="absolute start-0 top-full z-30 mt-2 flex max-h-80 w-full max-w-2xl flex-col gap-2 overflow-y-auto rounded-md border border-border-subtle bg-surface-1 p-3">
        {/* Announced rather than drawn as a bar: there is no completion to show for a request whose
          length nobody knows, and a determinate bar would be a number invented to fill a shape. */}
        {search.searching ? (
          <p className="text-body text-text-secondary" role="status">
            {t("discover.search.searching")}
          </p>
        ) : null}

        {!search.searching && search.failed ? (
          <ErrorState
            body={t("discover.searchFailed.body")}
            title={t("discover.searchFailed.title")}
          />
        ) : null}

        {!search.searching && !search.failed && search.results.length > 0 ? (
          <>
            {/* **Which of the two answered, said out loud.** The catalogue's own index is instant and
                offline; a provider answer is something the core has just written down. A reader who is
                told "in your catalogue" knows the title is one they can open right now, and one told
                "from your providers" knows a request was spent — and the two are otherwise
                indistinguishable on screen. */}
            <p className="text-caption text-text-tertiary">
              {t(
                search.source === "catalogue"
                  ? "discover.search.source.catalogue"
                  : "discover.search.source.provider",
              )}
            </p>
            <ul aria-busy={search.searching} className="flex flex-col gap-1">
              {search.results.map((row) => (
                <li key={row.id}>
                  {/* A link, because it is one: it says where it goes, and a keyboard user opens it with
                    `Enter` rather than activating a button. */}
                  <Link
                    href={routePath(routeForResult(row))}
                    onNavigate={() => {
                      onNavigate(routeForResult(row));
                    }}
                  >
                    {row.name}
                  </Link>
                  <span className="ml-2 text-caption text-text-tertiary">{kindForResult(row)}</span>
                </li>
              ))}
            </ul>
          </>
        ) : null}

        {!search.searching && !search.failed && search.searched && search.results.length === 0 ? (
          <EmptyState body={t("discover.none.body")} title={t("discover.none.title")} />
        ) : null}
      </div>
    ) : null;
  const toolbar = (
    <DiscoverToolbar
      search={search}
      rails={rails}
      titleKind={titleKind}
      onKindChange={(kind) => onKindChange?.(kind)}
      feedback={feedback}
      {...(trailing === undefined ? {} : { trailing })}
    />
  );

  return (
    <Screen title={t("discover.title")} className="[&>header]:sr-only">
      {slides.length === 0 ? (
        /* No slides is no hero — unless the first fetch is in the air, where the band is what keeps
           the screen from re-laying out when the hero arrives. The toolbar rides it either way, so
           the controls a person is already using do not move. */
        firstFetch ? (
          <DiscoverHeroSkeleton bleed toolbar={toolbar} />
        ) : (
          toolbar
        )
      ) : (
        <DiscoverHero
          bleed
          toolbar={toolbar}
          onOpen={(slide) => onNavigate({ id: slide.id, kind: "title", titleKind: slide.kind })}
          slides={slides}
          {...(onPlay === undefined ? {} : { onPlay })}
        />
      )}
      {rails.refreshError === null ? null : (
        <p className="text-caption text-state-danger" role="status">
          {t("discover.refresh.failed")}
        </p>
      )}
      {rails.failed ? (
        <ErrorState body={t("discover.failed.body")} title={t("discover.failed.title")} />
      ) : null}

      {/* The section's own line, said here rather than drawn as a spinner over the rails
          (`DESIGN_SYSTEM §11` rule 2). It is the core's answer, not a guess from an empty list: an
          empty rail is what a fetch in the air and a missing key both look like. */}
      {firstFetch ? (
        <p className="text-body text-text-secondary" role="status">
          {t("discover.fetching.body")}
        </p>
      ) : null}

      {rails.rails.length === 0 && !rails.failed && !unknownYet ? (
        <EmptyState
          body={t("discover.empty.body")}
          className="m-auto max-w-md items-center text-center"
          size="page"
          title={t("discover.empty.title")}
        />
      ) : null}

      {rails.rails.length === 0 ? null : (
        <div className="flex shrink-0 flex-col gap-6">
          {/* Nothing fetched yet, or a fetch that found nothing. Every rail below still draws its
              own empty state, so "no provider has answered yet" stays distinguishable from "this
              rail is empty on purpose". */}
          {nothingAtAll && !unknownYet ? (
            <EmptyState
              body={t("discover.empty.body")}
              className="max-w-md"
              size="page"
              title={t("discover.empty.title")}
            />
          ) : null}
          {scanned.length === 0 ? null : (
            <Rail
              empty={null}
              itemKey={(row) => String(row.id)}
              items={scanned}
              renderItem={(row, tabIndex) => (
                <TitleCard
                  detailsLabel={t("card.details")}
                  href={routePath({
                    kind: "title",
                    id: row.id,
                    titleKind: row.kind === "shows" ? "show" : "movie",
                  })}
                  meta={`${row.kind === "shows" ? t("discover.kind.show") : t("discover.kind.movie")} · ${t("library.tiles.items", { count: row.items })}`}
                  onNavigate={() => {
                    onNavigate({
                      kind: "title",
                      id: row.id,
                      titleKind: row.kind === "shows" ? "show" : "movie",
                    });
                  }}
                  overview={row.overview}
                  poster={imageUrl(row.poster)}
                  rating={row.rating}
                  ratingLabel={row.rating === null ? null : t("card.rating", { score: row.rating })}
                  showTitle
                  size="lg"
                  tabIndex={tabIndex}
                  title={row.name}
                  watchlistLabel={t("card.watchlisted")}
                  watchlisted={watchlisted.has(row.id)}
                />
              )}
              title={t("discover.rail.library")}
            />
          )}
          {saved.length === 0 ? null : (
            <Rail
              empty={null}
              itemKey={(row) => String(row.titleId)}
              items={saved}
              renderItem={(row, tabIndex) => (
                <TitleCard
                  detailsLabel={t("card.details")}
                  href={routePath({
                    kind: "title",
                    id: row.titleId,
                    titleKind: row.kind ?? "movie",
                  })}
                  onNavigate={() => {
                    onNavigate({
                      kind: "title",
                      id: row.titleId,
                      titleKind: row.kind ?? "movie",
                    });
                  }}
                  poster={imageUrl(row.poster)}
                  size="lg"
                  tabIndex={tabIndex}
                  title={row.title ?? t("discover.rail.unknown")}
                />
              )}
              title={t("discover.rail.watchlist")}
            />
          )}
          {watching.length === 0 ? null : (
            <Rail
              empty={null}
              itemKey={(row) => String(row.titleId)}
              items={watching}
              renderItem={(row, tabIndex) => {
                // Resume the episode the row names, now that a series row carries both numbers;
                // a whole-title row still opens the title's own file.
                const target =
                  row.seasonNumber !== null && row.episodeNumber !== null
                    ? {
                        kind: "player" as const,
                        id: row.titleId,
                        season: row.seasonNumber,
                        episode: row.episodeNumber,
                      }
                    : { kind: "player" as const, id: row.titleId };
                return (
                  <TitleCard
                    {...downloadCardProps(marks, row.titleId, t, progress)}
                    detailsLabel={t("card.details")}
                    href={routePath(target)}
                    meta={t("discover.continue.percent", {
                      percent: Math.round((row.percent ?? 0) * 100),
                    })}
                    onNavigate={() => {
                      onNavigate(target);
                    }}
                    poster={imageUrl(row.poster)}
                    size="lg"
                    tabIndex={tabIndex}
                    title={row.title ?? t("discover.rail.unknown")}
                  />
                );
              }}
              title={t("discover.rail.continueWatching")}
            />
          )}
          {rails.rails.map((rail) => (
            <Rail
              empty={
                /* While the first fetch is in the air the rails keep their geometry and stay
                   silent: the section's line above already says what is happening, and "add a
                   provider key in Settings" is the wrong sentence for a user who has one. */
                unknownYet ? null : (
                  <EmptyState
                    body={t("discover.rail.empty.body")}
                    size="inline"
                    title={t("discover.rail.empty.title")}
                  />
                )
              }
              itemKey={(item) => item.titleId}
              items={rail.items}
              key={rail.rail}
              /* A rail whose first fetch has not answered yet draws placeholders at the size of the
                 tiles that will replace them (`DESIGN_SYSTEM §11` rule 2), rather than eight empty
                 headings over a wall of nothing. A rail that already holds items keeps drawing them
                 while a refresh runs: stale posters are more use than placeholders. */
              {...(firstFetch ? { skeleton: "lg" as const } : {})}
              renderItem={(item, tabIndex) => (
                <TitleCard
                  {...downloadCardProps(marks, item.titleId, t, progress)}
                  detailsLabel={t("card.details")}
                  href={routePath(routeFor(item))}
                  meta={itemMeta(item, kindFor(item))}
                  onNavigate={() => {
                    onNavigate(routeFor(item));
                  }}
                  overview={item.overview}
                  poster={imageUrl(item.poster)}
                  rating={item.rating}
                  ratingLabel={ratingLabel(item)}
                  size="lg"
                  tabIndex={tabIndex}
                  title={item.name}
                  watchlistLabel={t("card.watchlisted")}
                  watchlisted={watchlisted.has(item.titleId)}
                />
              )}
              title={t(RAIL_HEADINGS[rail.rail] ?? "discover.rail.unknown")}
            />
          ))}
        </div>
      )}
    </Screen>
  );
}

/** Discover, wired to the core. */
export function DiscoverScreen({
  onNavigate,
  onBackdrops,
  watchlist,
  libraryTitles,
  titleKind,
  trailing,
  downloads,
}: {
  readonly onNavigate: (route: Route) => void;
  /** The featured title's backdrop, reported so the profile selector can draw the same picture. */
  readonly onBackdrops?: (urls: readonly string[]) => void;
  /** The selected type, or `undefined` for **All** — which is also what a bare `/` means. */
  readonly titleKind?: RailKindDto | undefined;
  readonly trailing?: ReactNode;
  readonly downloads?: DownloadMarks | undefined;
  /** The account's saved titles, drawn as the first rail. */
  readonly watchlist?: readonly WatchlistEntryDto[] | undefined;
  /** The account's own scanned titles, drawn as their own rail. */
  readonly libraryTitles?: readonly LibraryTitleDto[] | undefined;
}): ReactElement {
  const rails = useHomeRails(titleKind);
  // What this account watched most recently, drawn above the provider rails; one read per visit,
  // because the only writer is the player this same page opens.
  const continueWatching = useContinueWatching();
  // The box's text lives in the hook, because the debounce does: a screen that held it would have to
  // run the timer itself, and the "catalogue first" rule would be split across two files.
  const { failed, results, searched, searching, source, query, setQuery, searchNow } =
    useTitleSearch(titleKind);

  return (
    <DiscoverView
      downloads={downloads}
      watchlist={watchlist}
      libraryTitles={libraryTitles}
      onBackdrops={onBackdrops}
      onNavigate={onNavigate}
      titleKind={titleKind}
      {...(trailing === undefined ? {} : { trailing })}
      onKindChange={(kind) => {
        // All is the absence of the parameter rather than `type=all`: `/` is the home screen, and a
        // URL that spelled it would be a second spelling of the same place.
        onNavigate(
          kind === undefined ? { kind: "discover" } : { kind: "discover", titleKind: kind },
        );
        // Nothing to re-run here: the hook takes the kind as an argument, so the change re-runs the
        // search through its own effect — one place owns that rule rather than two.
      }}
      rails={rails}
      continueWatching={continueWatching}
      search={{
        results,
        searching,
        failed,
        searched,
        source,
        query,
        setQuery,
        searchNow,
      }}
    />
  );
}
