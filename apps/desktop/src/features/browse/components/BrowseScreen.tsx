/**
 * Browse: the filtered grid — every avenue into the catalogue that a rail is not.
 *
 * **A rail is a list the provider publishes; this is a list the reader composes.** Discover draws
 * seven fixed lists because that is what a home screen is for, and none of them can answer "the
 * documentary films rated eight or better, newest first" — a rail takes no arguments
 * (`peregrine_providers::MetadataProvider::rail`). Browse is the screen for that question, and the
 * whole of it is the query: a kind, a genre, a floor under the score, and an order.
 *
 * **No business rule lives here** (`AGENTS R7`). Which titles are on a page, whether another page
 * follows, whether the score floor is meaningful, and how a provider maps an order onto its own
 * parameter all arrive as answers; this screen picks the tile geometry, draws the controls, and asks
 * for the next page when the grid says the reader has reached the end. **The search field is a
 * search of everything rather than a narrowing of the page.** A browse page is asked of a provider by
 * filters and carries no text parameter, so a local filter could only ever find what had already
 * arrived — and its index tokenises "Spider-Man" into two words, which is why "spiderman" matched
 * nothing. The field asks the providers instead (falling back to the catalogue's own index when none
 * is configured), and the answer is drawn above the grid's empty space while the query is live.
 *
 * **The two halves are split so the drawing can be tested.** [`BrowseScreen`] is the data layer — two
 * hooks and the route's kind — and [`BrowseView`] takes what they answered as props, which is what
 * lets a Node test render the grid, the toolbar, and the empty states from synthetic DTOs with no
 * bridge, no store, and no network.
 */

import {
  Button,
  EmptyState,
  ErrorState,
  Grid,
  Icon,
  Screen,
  Skeleton,
  TextInput,
  TitleCard,
} from "@peregrine/ui";
import type { BrowseSortDto, GenreDto, RailItemDto } from "@peregrine/ipc-client";
import { useMemo, type ReactElement } from "react";
import { useTranslation } from "react-i18next";

import { routePath, type Route, type TitleKind } from "../../../app/routes.js";
import {
  downloadCardProps,
  downloadsFirst,
  rankByDownload,
  type DownloadMarks,
  type DownloadTitleSource,
} from "../../../lib/download-status.js";
import { useDownloadProgress } from "../../../lib/download-progress.js";
import { imageUrl } from "../../../lib/media.js";
import { MIN_QUERY_CHARS, useTitleSearch, type TileSearch } from "../../../lib/title-search.js";
import { useBrowse, type BrowseData, type BrowseFilters } from "../hooks/useBrowse.js";
import { useBrowseGenres } from "../hooks/useBrowseGenres.js";

/** The orders the toolbar offers, in the order it lists them. */
const SORTS = ["popularity", "rating", "newest", "oldest"] as const;

/** The score floors the toolbar offers, out of ten. */
const FLOORS = [5, 6, 7, 8, 9] as const;

/** Nothing downloading, for the view drawn from synthetic props in a test. */
const NO_DOWNLOADS: DownloadMarks = new Map();

/**
 * A `lg` tile's width, from §4's ladder, and the row height that goes with it.
 *
 * A row is the poster's height plus the gap, and the poster's height is the width over the 2:3
 * aspect every card keeps — the same arithmetic the gallery's grid uses. Fixed numbers rather than a
 * measurement, because that is what makes the grid's window arithmetic (`packages/ui`'s `Grid`).
 */
const TILE_WIDTH = 176;
const TILE_POSTER_HEIGHT = TILE_WIDTH * 1.5;
const TILE_ROW_HEIGHT = TILE_POSTER_HEIGHT + 12;

/**
 * The shape of a filter control: a pill, like the rest of the toolbar row.
 *
 * `md:flex-1` gives each filter one share of the row once there is width for it, and the search pill
 * takes two (`md:grow-[2]`) because a title is longer than the word "Newest". `min-w-0` is required
 * for the shares to be exact: a `select`'s automatic minimum is its widest option, which would
 * otherwise refuse to shrink.
 */
const SELECT_CLASS =
  "h-9 min-w-0 rounded-full border border-border-subtle bg-surface-0/70 px-3 text-body text-text-primary disabled:opacity-40 md:flex-1";

/** What the drawing needs, which is what the hooks answered plus the route's kind. */
export interface BrowseViewProps {
  /** The kind the route names, defaulted by the screen above when it names none. */
  readonly kind: TitleKind;
  readonly data: BrowseData;
  /** The provider's genres for `kind`; empty until they arrive, and empty is still browsable. */
  readonly genres: readonly GenreDto[];
  /** The title search field's state, which replaces the grid while its query is live. */
  readonly search: TileSearch;
  readonly onKind: (kind: TitleKind) => void;
  readonly onNavigate: (route: Route) => void;
  /** A download status per title, so a tile wears the mark of a transfer in flight. */
  readonly downloads?: DownloadMarks | undefined;
  /** The queue rows, which is what puts a download above the provider's page. */
  readonly downloadRows?: readonly DownloadTitleSource[] | undefined;
  /** Titles on the account's watchlist, so their tiles carry the bookmark. */
  readonly watchlisted?: ReadonlySet<number> | undefined;
  /** The shell's back control, present only when there is somewhere behind this screen. */
  readonly onBack?: (() => void) | undefined;
  readonly backLabel?: string | undefined;
}

/** The screen as it is drawn, from answers. */
export function BrowseView({
  kind,
  data,
  genres,
  search,
  onKind,
  onNavigate,
  downloads,
  downloadRows,
  watchlisted,
  onBack,
  backLabel,
}: BrowseViewProps): ReactElement {
  const { t } = useTranslation();
  const marks = downloads ?? NO_DOWNLOADS;
  const progress = useDownloadProgress();
  // The field hands its text to the search hook, which owns the debounce, the request, and the three
  // answers. This screen only decides which surface the answer replaces.
  const searchMode = search.query.trim().length >= MIN_QUERY_CHARS;

  const routeFor = (item: RailItemDto): Route => ({
    id: item.titleId,
    kind: "title",
    titleKind: item.kind,
  });
  const kindFor = (item: RailItemDto): string =>
    item.kind === "movie" ? t("discover.kind.movie") : t("discover.kind.show");
  const ratingLabel = (item: RailItemDto): string | null =>
    item.rating === null ? null : t("card.rating", { score: item.rating });

  const change = (part: Partial<BrowseFilters>): void => {
    data.setFilters({ ...data.filters, ...part });
  };

  // One tile drawing, two grids: the browse page and a live search answer with the same row
  // shape, so the same card, marks, and geometry are what a reader sees in both.
  const renderTile = (item: RailItemDto): ReactElement => (
    <TitleCard
      detailsLabel={t("card.details")}
      {...downloadCardProps(marks, item.titleId, t, progress)}
      href={routePath(routeFor(item))}
      // A grid has no room to give the 30 % enlargement, so the panel reveals inside the tile
      // instead of scaling the first and last columns under the scroller's edge.
      lift={false}
      meta={item.year === null ? kindFor(item) : `${String(item.year)} · ${kindFor(item)}`}
      onNavigate={() => {
        onNavigate(routeFor(item));
      }}
      overview={item.overview}
      poster={imageUrl(item.poster)}
      rating={item.rating}
      ratingLabel={ratingLabel(item)}
      size="lg"
      title={item.name}
      watchlistLabel={t("card.watchlisted")}
      watchlisted={watchlisted?.has(item.titleId) ?? false}
    />
  );

  const toolbar = (
    <div className="flex flex-wrap items-center gap-3" data-browse-toolbar="">
      <form
        role="search"
        // **The pill is the field, so the pill draws the focus ring.** `focusRing="none"` on the
        // input keeps a second rounded outline from appearing inside this one — the rule Discover's
        // search box already follows. `basis-full` gives the field its own row before the toolbar
        // wraps at narrow widths; at `md` and up it takes **two shares** to the filters' one, because
        // a title is longer than the word "Newest" and the field is the screen's reason to exist.
        className="flex h-9 min-w-0 basis-full items-center gap-2 rounded-full border border-border-subtle bg-surface-0/70 px-3 focus-within:outline-2 focus-within:outline-offset-2 focus-within:outline-focus-ring md:basis-0 md:grow-[2]"
        onSubmit={(event) => {
          // Typing already searches; Enter is the escape hatch from the debounce. The default still
          // has to be stopped, because an implicit submit would navigate the webview away.
          event.preventDefault();
          search.searchNow();
        }}
      >
        <Icon className="text-text-tertiary" name="search" size={16} />
        <TextInput
          autoComplete="off"
          className="h-full min-w-0 flex-1 [&_input]:h-9 [&_input]:rounded-full [&_input]:border-0 [&_input]:bg-transparent"
          focusRing="none"
          label={t("browse.search.label")}
          labelHidden
          name="query"
          onChange={(event) => {
            search.setQuery(event.target.value);
          }}
          placeholder={t("browse.search.placeholder")}
          type="search"
          value={search.query}
        />
      </form>

      {/* **Two choices and no All.** Discover can offer All because every rail is drawn whatever the
          kind, but a provider's queried list is one kind — there is no endpoint that answers a mixed
          page — so a control offering All would be a control that could not be honoured. A bare
          `/browse` is therefore the default kind, and the first press of the other one makes the
          URL say which is on screen. */}
      <div
        aria-label={t("browse.kind.label")}
        className="flex h-9 min-w-0 items-center gap-1 rounded-full border border-border-subtle bg-surface-0/70 p-1 md:flex-1"
        role="group"
      >
        {(["movie", "show"] as const).map((option) => (
          <Button
            aria-pressed={kind === option}
            // The two choices split their group's inner height and width, so the segmented control
            // is exactly as tall as the search pill and the selects beside it.
            className="h-full flex-1 rounded-full px-1"
            key={option}
            onClick={() => {
              onKind(option);
            }}
            variant={kind === option ? "secondary" : "ghost"}
          >
            {t(option === "movie" ? "discover.filter.movies" : "discover.filter.shows")}
          </Button>
        ))}
      </div>

      {/* **The three provider filters belong to the grid, and a live search is not the grid.** They
          are disabled rather than left live, because a change while searching would reload a page
          nobody can see and spend a provider request on it. The kind stays live: it is part of the
          search request. */}
      <select
        aria-label={t("browse.genre.label")}
        className={SELECT_CLASS}
        disabled={searchMode}
        onChange={(event) => {
          change({ genre: event.target.value === "" ? null : Number(event.target.value) });
        }}
        value={data.filters.genre ?? ""}
      >
        <option value="">{t("browse.genre.all")}</option>
        {genres.map((genre) => (
          <option key={genre.id} value={genre.id}>
            {genre.name}
          </option>
        ))}
      </select>

      <select
        aria-label={t("browse.sort.label")}
        className={SELECT_CLASS}
        disabled={searchMode}
        onChange={(event) => {
          change({ sort: event.target.value as BrowseSortDto });
        }}
        value={data.filters.sort}
      >
        {SORTS.map((sort) => (
          <option key={sort} value={sort}>
            {t(`browse.sort.${sort}`)}
          </option>
        ))}
      </select>

      <select
        aria-label={t("browse.rating.label")}
        className={SELECT_CLASS}
        disabled={searchMode}
        onChange={(event) => {
          change({ minRating: event.target.value === "" ? null : Number(event.target.value) });
        }}
        value={data.filters.minRating ?? ""}
      >
        <option value="">{t("browse.rating.any")}</option>
        {FLOORS.map((floor) => (
          <option key={floor} value={floor}>
            {t("browse.rating.floor", { score: floor })}
          </option>
        ))}
      </select>
    </div>
  );

  // **A download leads the whole screen, not just the page.** `rankByDownload` can only move a
  // title the provider already sent; `downloadsFirst` merges the queue's rows in, so a completed or
  // in-progress title opens Browse at the top even when the provider's page does not carry it. A
  // title that is both keeps the provider's richer row and simply moves; a failed job does not hoist.
  const items = useMemo(() => {
    if (downloadRows === undefined || downloadRows.length === 0) {
      return rankByDownload(data.items, marks);
    }
    return downloadsFirst(data.items, downloadRows, kind, (row) => ({
      titleId: row.titleId,
      kind,
      // `downloadsFirst` only builds a row whose title is not null, so this cannot be an empty name.
      name: row.title ?? "",
      year: null,
      poster: row.poster,
      backdrop: null,
      playable: row.state === "seeding" || row.state === "completed",
      rating: null,
      overview: null,
      votes: null,
    }));
  }, [data.items, marks, downloadRows, kind]);
  const nothing = items.length === 0;

  return (
    <Screen backLabel={backLabel} onBack={onBack} title={t("browse.title")}>
      {toolbar}

      {/* The browse page's failure belongs to the grid. A live search has its own failure in its own
          panel, and showing this one above successful results would say the search broke. */}
      {data.failed === null || searchMode ? null : (
        <ErrorState body={t("browse.failed.body")} title={t("browse.failed.title")} />
      )}

      {searchMode ? (
        // **A live query replaces the grid with its own poster grid.** Same tile, marks, and geometry
        // as the browse page, because a search result is a catalogue row with the year, the score,
        // and the cached poster: `search_tiles` writes it through the list path. The text is the
        // grid's reset key, so a new query starts from its first tile.
        <>
          {search.failed ? (
            <ErrorState
              body={t("discover.searchFailed.body")}
              title={t("discover.searchFailed.title")}
            />
          ) : null}

          {search.searching || (!search.searched && !search.failed) ? (
            <p className="text-body text-text-secondary" role="status">
              {t("discover.search.searching")}
            </p>
          ) : null}

          {search.results.length > 0 ? (
            <div className="min-h-0 flex-1" data-browse-search="">
              <Grid
                itemKey={(item) => item.titleId}
                items={search.results}
                label={t("browse.search.results")}
                // A search is not paged: the provider answered once and there is no next page to
                // ask for.
                resetKey={search.query}
                renderItem={renderTile}
                rowHeight={TILE_ROW_HEIGHT}
                tileWidth={TILE_WIDTH}
              />
            </div>
          ) : !search.searching && !search.failed && search.searched ? (
            <EmptyState
              body={t("discover.none.body")}
              className="m-auto max-w-md items-center text-center"
              size="page"
              title={t("discover.none.title")}
            />
          ) : null}
        </>
      ) : nothing ? (
        // Nothing at all with no failure is a real answer — the filters exclude everything, or the
        // provider has nothing here — so it is an empty state and not a spinner or a blank body.
        data.loading || data.failed !== null ? null : (
          <EmptyState
            body={t("browse.empty.body")}
            className="m-auto max-w-md items-center text-center"
            size="page"
            title={t("browse.empty.title")}
          />
        )
      ) : (
        // `flex-1 min-h-0` is load-bearing rather than cosmetic: the grid scrolls itself, so a
        // parent that grew with its content would make the whole grid the "viewport" and draw every
        // tile (`packages/ui`'s `Grid`).
        <div className="min-h-0 flex-1">
          <Grid
            endThreshold={8}
            itemKey={(item) => item.titleId}
            items={items}
            label={t("browse.grid.label")}
            // **The placeholders are tiles, in the grid's own flow.** A reader who has scrolled to
            // the end of what is loaded has to be able to tell "there is nothing more" from "the
            // next page is still coming", and a line of text under a wall of posters is easy to
            // miss — so the reservation is drawn where the arriving tiles will be, at their
            // geometry, and the sweep says the rest. `browse.loading` and `browse.end` below are the
            // same fact for a screen reader, which cannot see a skeleton (`Skeleton` is
            // `aria-hidden` on purpose).
            loading={data.loading}
            onEndReached={data.loadMore}
            placeholder={
              <Skeleton
                className="rounded-md"
                style={{ height: TILE_POSTER_HEIGHT, width: TILE_WIDTH }}
              />
            }
            placeholderRows={1}
            // A new query starts at the top: the scroller is the same element across a filter
            // change, so without this the new answer would be drawn from wherever the previous one
            // was scrolled to — which is what made switching tabs look broken.
            resetKey={data.queryKey}
            renderItem={renderTile}
            rowHeight={TILE_ROW_HEIGHT}
            tileWidth={TILE_WIDTH}
          />
        </div>
      )}

      {/* The footer belongs to the grid. A page in the air and the end of the list are the two
          answers a reader is looking for; "more to come" is the third, and it needs no line because
          scrolling finds it. A live search draws its own state above instead. */}
      {searchMode ? null : data.loading && !nothing ? (
        <p className="text-caption text-text-tertiary" role="status">
          {t("browse.loading")}
        </p>
      ) : !data.loading && !data.hasMore && !nothing ? (
        <p className="text-caption text-text-tertiary" role="status">
          {t("browse.end")}
        </p>
      ) : null}
    </Screen>
  );
}

/** Browse, wired to the core. */
export function BrowseScreen({
  onNavigate,
  titleKind,
  downloads,
  downloadRows,
  watchlisted,
  onBack,
  backLabel,
}: {
  readonly onNavigate: (route: Route) => void;
  /** The kind the route names, or `undefined` for the default — see `BrowseView`'s toolbar. */
  readonly titleKind?: TitleKind | undefined;
  readonly downloads?: DownloadMarks | undefined;
  readonly downloadRows?: readonly DownloadTitleSource[] | undefined;
  readonly watchlisted?: ReadonlySet<number> | undefined;
  readonly onBack?: (() => void) | undefined;
  readonly backLabel?: string | undefined;
}): ReactElement {
  // The map spells the row `/browse/:type?`, so the parameter is optional; what it defaults to is
  // the one kind a provider's queried list can be drawn for. A bare `/browse` is therefore Films,
  // and choosing TV shows moves the window to `/browse/show` rather than leaving the URL silent
  // about what is on screen.
  const kind: TitleKind = titleKind ?? "movie";
  const data = useBrowse(kind);
  const genres = useBrowseGenres(kind);
  // **The tile-shaped provider search, unlike Discover's dropdown.** A browse page carries no text
  // parameter, so a hit can only come from the catalogue's index or a provider; the index tokenises
  // "Spider-Man" into two words and cannot match "spiderman", while the provider that published the
  // title can. `Runtime::search_tiles` writes that answer through the list writer before returning
  // it, so a result is a catalogue row carrying the poster a tile draws.
  const search = useTitleSearch(kind, { tiles: true });

  return (
    <BrowseView
      backLabel={backLabel}
      data={data}
      downloadRows={downloadRows}
      downloads={downloads}
      genres={genres}
      kind={kind}
      search={search}
      watchlisted={watchlisted}
      onBack={onBack}
      onKind={(selected) => {
        onNavigate({ kind: "browse", titleKind: selected });
      }}
      onNavigate={onNavigate}
    />
  );
}
