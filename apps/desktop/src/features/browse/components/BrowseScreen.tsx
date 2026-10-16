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
 * for the next page when the grid says the reader has reached the end. Search is nowhere on this
 * screen on purpose: the catalogue's own search is `PG-E8`'s screen, and a second search box here
 * would be the same question asked twice.
 *
 * **The two halves are split so the drawing can be tested.** [`BrowseScreen`] is the data layer — two
 * hooks and the route's kind — and [`BrowseView`] takes what they answered as props, which is what
 * lets a Node test render the grid, the toolbar, and the empty states from synthetic DTOs with no
 * bridge, no store, and no network.
 */

import { Button, EmptyState, ErrorState, Grid, Screen, Skeleton, TitleCard } from "@peregrine/ui";
import type { BrowseSortDto, GenreDto, RailItemDto } from "@peregrine/ipc-client";
import type { ReactElement } from "react";
import { useTranslation } from "react-i18next";

import { routePath, type Route, type TitleKind } from "../../../app/routes.js";
import { imageUrl } from "../../../lib/media.js";
import { useBrowse, type BrowseData, type BrowseFilters } from "../hooks/useBrowse.js";
import { useBrowseGenres } from "../hooks/useBrowseGenres.js";

/** The orders the toolbar offers, in the order it lists them. */
const SORTS = ["popularity", "rating", "newest", "oldest"] as const;

/** The score floors the toolbar offers, out of ten. */
const FLOORS = [5, 6, 7, 8, 9] as const;

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

/** The shape of a filter control: a pill, like the rest of the toolbar row. */
const SELECT_CLASS =
  "h-9 rounded-full border border-border-subtle bg-surface-0/70 px-3 text-body text-text-primary";

/** What the drawing needs, which is what the hooks answered plus the route's kind. */
export interface BrowseViewProps {
  /** The kind the route names, defaulted by the screen above when it names none. */
  readonly kind: TitleKind;
  readonly data: BrowseData;
  /** The provider's genres for `kind`; empty until they arrive, and empty is still browsable. */
  readonly genres: readonly GenreDto[];
  readonly onKind: (kind: TitleKind) => void;
  readonly onNavigate: (route: Route) => void;
}

/** The screen as it is drawn, from answers. */
export function BrowseView({
  kind,
  data,
  genres,
  onKind,
  onNavigate,
}: BrowseViewProps): ReactElement {
  const { t } = useTranslation();

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

  const toolbar = (
    <div className="flex flex-wrap items-center gap-3" data-browse-toolbar="">
      {/* **Two choices and no All.** Discover can offer All because every rail is drawn whatever the
          kind, but a provider's queried list is one kind — there is no endpoint that answers a mixed
          page — so a control offering All would be a control that could not be honoured. A bare
          `/browse` is therefore the default kind, and the first press of the other one makes the
          URL say which is on screen. */}
      <div
        aria-label={t("browse.kind.label")}
        className="flex items-center gap-1 rounded-full border border-border-subtle bg-surface-0/70 p-1"
        role="group"
      >
        {(["movie", "show"] as const).map((option) => (
          <Button
            aria-pressed={kind === option}
            className="rounded-full"
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

      <select
        aria-label={t("browse.genre.label")}
        className={SELECT_CLASS}
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

  const nothing = data.items.length === 0;

  return (
    <Screen title={t("browse.title")}>
      {toolbar}

      {data.failed === null ? null : (
        <ErrorState body={t("browse.failed.body")} title={t("browse.failed.title")} />
      )}

      {nothing ? (
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
        <>
          {/* `flex-1 min-h-0` is load-bearing rather than cosmetic: the grid scrolls itself, so a
              parent that grew with its content would make the whole grid the "viewport" and draw every
              tile (`packages/ui`'s `Grid`). */}
          <div className="min-h-0 flex-1">
            <Grid
              endThreshold={8}
              itemKey={(item) => item.titleId}
              items={data.items}
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
              renderItem={(item) => (
                <TitleCard
                  detailsLabel={t("card.details")}
                  href={routePath(routeFor(item))}
                  meta={
                    item.year === null ? kindFor(item) : `${String(item.year)} · ${kindFor(item)}`
                  }
                  onNavigate={() => {
                    onNavigate(routeFor(item));
                  }}
                  overview={item.overview}
                  poster={imageUrl(item.poster)}
                  rating={item.rating}
                  ratingLabel={ratingLabel(item)}
                  size="lg"
                  title={item.name}
                />
              )}
              rowHeight={TILE_ROW_HEIGHT}
              tileWidth={TILE_WIDTH}
            />
          </div>

          {/* The footer is where the two states are said in words, and it is outside the scroller so
              it stays readable while the grid moves. A page in the air and the end of the list are
              the two answers a reader is looking for; "more to come" is the third, and it needs no
              line because scrolling finds it. */}
          {data.loading ? (
            <p className="text-caption text-text-tertiary" role="status">
              {t("browse.loading")}
            </p>
          ) : data.hasMore ? null : (
            <p className="text-caption text-text-tertiary" role="status">
              {t("browse.end")}
            </p>
          )}
        </>
      )}
    </Screen>
  );
}

/** Browse, wired to the core. */
export function BrowseScreen({
  onNavigate,
  titleKind,
}: {
  readonly onNavigate: (route: Route) => void;
  /** The kind the route names, or `undefined` for the default — see `BrowseView`'s toolbar. */
  readonly titleKind?: TitleKind | undefined;
}): ReactElement {
  // The map spells the row `/browse/:type?`, so the parameter is optional; what it defaults to is
  // the one kind a provider's queried list can be drawn for. A bare `/browse` is therefore Films,
  // and choosing TV shows moves the window to `/browse/show` rather than leaving the URL silent
  // about what is on screen.
  const kind: TitleKind = titleKind ?? "movie";
  const data = useBrowse(kind);
  const genres = useBrowseGenres(kind);

  return (
    <BrowseView
      data={data}
      genres={genres}
      kind={kind}
      onKind={(selected) => {
        onNavigate({ kind: "browse", titleKind: selected });
      }}
      onNavigate={onNavigate}
    />
  );
}
