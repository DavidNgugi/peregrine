import {
  Button,
  EmptyState,
  ErrorState,
  Link,
  Rail,
  Screen,
  TextInput,
  TitleCard,
} from "@peregrine/ui";
import type { RailItemDto, TitleMatchDto } from "@peregrine/ipc-client";
import { useState, type ReactElement } from "react";
import { useTranslation } from "react-i18next";

import { routePath, type Route } from "../../../app/routes.js";
import { imageUrl } from "../../../lib/media.js";
import { useDiscoverSearch } from "../hooks/useDiscoverSearch.js";
import { useHomeRails, type HomeRailsView } from "../hooks/useHomeRails.js";
import { DiscoverHero, type HeroSlide } from "./DiscoverHero.js";
import { heroSlides } from "./hero-rotation.js";

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
 * **The search box is at the top, above the hero**, which is where a person looks for it first: the
 * form is the same form, moved rather than rebuilt, and the answers it produces stay beside it — a
 * box at the top whose results appeared below the rails would be a box whose results nobody finds.
 * It is submitted rather than typed: it leaves the machine, and a request per keystroke is a request
 * per keystroke against somebody else's API (`AGENTS R24`). It stays because a rail is what a
 * provider chose to publish and a search is what the person came for.
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
  readonly search: {
    readonly results: readonly TitleMatchDto[];
    readonly searching: boolean;
    readonly failed: boolean;
    readonly searched: boolean;
    readonly submit: (query: string) => void;
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
}

/** The screen as it is drawn, from answers. */
export function DiscoverView({
  rails,
  search,
  onPlay,
  onNavigate,
}: DiscoverViewProps): ReactElement {
  const { i18n, t } = useTranslation();

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
  // The two spoken forms the tile's name carries, and the panel repeats: the score beside its star
  // and the count beside it. `null` is the core's "the provider said nothing", which draws and
  // announces nothing rather than a zero nobody claimed. The count is grouped by the locale the
  // screen is in, because i18next interpolates a number as it is and "24000 votes" is not a number
  // anybody writes.
  const ratingLabel = (item: RailItemDto): string | null =>
    item.rating === null ? null : t("card.rating", { score: item.rating });
  const votesLabel = (item: RailItemDto): string | null =>
    item.votes === null
      ? null
      : t("card.votes", {
          count: item.votes,
          formatted: new Intl.NumberFormat(i18n.language).format(item.votes),
        });

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
    votesLabel: votesLabel(item),
    playable: item.playable,
  }));

  const nothingAtAll = rails.rails.every((rail) => rail.items.length === 0);
  // The one state the screen says out loud rather than drawing as an empty catalogue: a provider is
  // configured and the fetch is in the air, so the answer is "coming", not "nothing" (`FLOWS F2`).
  const firstFetch = rails.state === "fetching";

  return (
    <Screen title={t("discover.title")}>
      {/* The search box, and everything it answers, at the top of the screen: this is where the
          reference puts it and where a person looks for it. */}
      <form
        className="flex items-end gap-2"
        onSubmit={(event) => {
          event.preventDefault();
          search.submit(search.query);
        }}
      >
        <TextInput
          className="max-w-md flex-1"
          label={t("discover.search.label")}
          onChange={(event) => {
            search.setQuery(event.target.value);
          }}
          placeholder={t("discover.search.placeholder")}
          value={search.query}
        />
        <Button disabled={search.searching || search.query.trim() === ""} type="submit">
          {t("discover.search.action")}
        </Button>
      </form>

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
      ) : null}

      {!search.searching && !search.failed && search.searched && search.results.length === 0 ? (
        <EmptyState body={t("discover.none.body")} title={t("discover.none.title")} />
      ) : null}

      <div className="flex flex-wrap items-center gap-2">
        <Button
          disabled={rails.refreshing}
          onClick={() => {
            void rails.refresh();
          }}
          variant="secondary"
        >
          {rails.refreshing ? t("discover.refresh.running") : t("discover.refresh.action")}
        </Button>
        {/* A failed start is reported where the control is: the rails below are still the last good
            answer, and replacing them with an error would throw away what did arrive. */}
        {rails.refreshError === null ? null : (
          <p className="text-caption text-state-danger" role="status">
            {t("discover.refresh.failed")}
          </p>
        )}
      </div>

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

      {/* No slides is no hero: an empty catalogue draws its empty state below rather than a band of
          nothing above it, and one slide draws itself without a rotation (`hero-rotation.ts`). */}
      {slides.length === 0 ? null : (
        <DiscoverHero
          onOpen={(slide) => {
            onNavigate({ id: slide.id, kind: "title", titleKind: slide.kind });
          }}
          slides={slides}
          {...(onPlay === undefined ? {} : { onPlay })}
        />
      )}

      {rails.rails.length === 0 && !rails.failed && !firstFetch ? (
        <EmptyState
          body={t("discover.empty.body")}
          className="m-auto max-w-md items-center text-center"
          size="page"
          title={t("discover.empty.title")}
        />
      ) : null}

      {rails.rails.length === 0 ? null : (
        <div className="flex flex-col gap-6">
          {/* Nothing fetched yet, or a fetch that found nothing. Every rail below still draws its
              own empty state, so "no provider has answered yet" stays distinguishable from "this
              rail is empty on purpose". */}
          {nothingAtAll && !firstFetch ? (
            <EmptyState
              body={t("discover.empty.body")}
              className="max-w-md"
              size="page"
              title={t("discover.empty.title")}
            />
          ) : null}
          {rails.rails.map((rail) => (
            <Rail
              empty={
                /* While the first fetch is in the air the rails keep their geometry and stay
                   silent: the section's line above already says what is happening, and "add a
                   provider key in Settings" is the wrong sentence for a user who has one. */
                firstFetch ? null : (
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
              renderItem={(item, tabIndex) => (
                <TitleCard
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
                  size="sm"
                  tabIndex={tabIndex}
                  title={item.name}
                  votesLabel={votesLabel(item)}
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
}: {
  readonly onNavigate: (route: Route) => void;
}): ReactElement {
  const rails = useHomeRails();
  const { failed, results, searched, searching, search } = useDiscoverSearch();
  const [query, setQuery] = useState("");

  return (
    <DiscoverView
      onNavigate={onNavigate}
      rails={rails}
      search={{
        results,
        searching,
        failed,
        searched,
        query,
        setQuery,
        submit: (text) => {
          void search(text);
        },
      }}
    />
  );
}
