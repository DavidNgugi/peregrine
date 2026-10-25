import { EmptyState, Grid, Icon, Screen, TextInput, TitleCard } from "@peregrine/ui";
import type { WatchlistEntryDto } from "@peregrine/ipc-client";
import { useMemo, useState, type ReactElement } from "react";
import { useTranslation } from "react-i18next";

import { routePath, type Route, type TitleKind } from "../../../app/routes.js";
import { imageUrl } from "../../../lib/media.js";
import type { WatchlistState } from "../hooks/useWatchlist.js";

const TILE_WIDTH = 176;
const TILE_ROW_HEIGHT = 276;

type Sort = "newest" | "oldest" | "title";

/** The account's saved catalogue, using Browse's grid and controls for a personal collection. */
export function MyListScreen({
  watchlist,
  onNavigate,
  onBack,
  backLabel,
}: {
  readonly watchlist: WatchlistState;
  readonly onNavigate: (route: Route) => void;
  readonly onBack?: (() => void) | undefined;
  readonly backLabel?: string | undefined;
}): ReactElement {
  const { t } = useTranslation();
  const [query, setQuery] = useState("");
  const [kind, setKind] = useState<TitleKind | "all">("all");
  const [sort, setSort] = useState<Sort>("newest");

  const items = useMemo(() => {
    const needle = query.trim().toLocaleLowerCase();
    return [...watchlist.rows]
      .filter((row) => {
        const matchesQuery =
          needle.length === 0 || (row.title ?? "").toLocaleLowerCase().includes(needle);
        const matchesKind = kind === "all" || row.kind === kind;
        return matchesQuery && matchesKind;
      })
      .sort((left, right) => {
        if (sort === "title") return (left.title ?? "").localeCompare(right.title ?? "");
        return sort === "newest" ? right.addedAt - left.addedAt : left.addedAt - right.addedAt;
      });
  }, [kind, query, sort, watchlist.rows]);

  return (
    <Screen backLabel={backLabel} onBack={onBack} title={t("discover.rail.watchlist")}>
      <div className="flex flex-wrap items-center gap-3">
        <form
          className="flex h-9 min-w-0 basis-full items-center gap-2 rounded-full border border-border-subtle bg-surface-0/70 px-3 focus-within:outline-2 focus-within:outline-offset-2 focus-within:outline-focus-ring md:basis-0 md:grow-[2]"
          onSubmit={(event) => event.preventDefault()}
          role="search"
        >
          <Icon className="text-text-tertiary" name="search" size={16} />
          <TextInput
            autoComplete="off"
            className="h-full min-w-0 flex-1 [&_input]:h-9 [&_input]:rounded-full [&_input]:border-0 [&_input]:bg-transparent"
            focusRing="none"
            label={t("myList.search.label")}
            labelHidden
            onChange={(event) => setQuery(event.target.value)}
            placeholder={t("myList.search.placeholder")}
            type="search"
            value={query}
          />
        </form>
        <div
          aria-label={t("browse.kind.label")}
          className="flex h-9 min-w-0 items-center gap-1 rounded-full border border-border-subtle bg-surface-0/70 p-1 md:flex-1"
          role="group"
        >
          {(["all", "movie", "show"] as const).map((option) => (
            <button
              aria-pressed={kind === option}
              className="h-full flex-1 rounded-full px-2 text-body text-text-primary focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus-ring"
              key={option}
              onClick={() => setKind(option)}
              type="button"
            >
              {t(
                option === "all"
                  ? "myList.filter.all"
                  : option === "movie"
                    ? "discover.filter.movies"
                    : "discover.filter.shows",
              )}
            </button>
          ))}
        </div>
        <select
          aria-label={t("myList.sort.label")}
          className="h-9 min-w-0 rounded-full border border-border-subtle bg-surface-0/70 px-3 text-body text-text-primary focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus-ring md:flex-1"
          onChange={(event) => setSort(event.target.value as Sort)}
          value={sort}
        >
          <option value="newest">{t("browse.sort.newest")}</option>
          <option value="oldest">{t("browse.sort.oldest")}</option>
          <option value="title">{t("myList.sort.title")}</option>
        </select>
      </div>

      {watchlist.failed !== null ? (
        <EmptyState body={t("myList.failed.body")} title={t("myList.failed.title")} />
      ) : items.length === 0 ? (
        <EmptyState body={t("myList.empty.body")} title={t("myList.empty.title")} />
      ) : (
        <div className="min-h-0 flex-1">
          <Grid
            itemKey={(item) => item.titleId}
            items={items}
            label={t("myList.grid.label")}
            rowHeight={TILE_ROW_HEIGHT}
            tileWidth={TILE_WIDTH}
            renderItem={(item: WatchlistEntryDto) => {
              const titleKind: TitleKind = item.kind === "show" ? "show" : "movie";
              return (
                <TitleCard
                  detailsLabel={t("card.details")}
                  href={routePath({ kind: "title", id: item.titleId, titleKind })}
                  lift={false}
                  meta={t(titleKind === "movie" ? "discover.kind.movie" : "discover.kind.show")}
                  onNavigate={() => onNavigate({ kind: "title", id: item.titleId, titleKind })}
                  overview={null}
                  poster={imageUrl(item.poster)}
                  rating={null}
                  ratingLabel={null}
                  size="lg"
                  title={item.title ?? t("myList.unknown")}
                  watchlistLabel={t("card.watchlisted")}
                  watchlisted
                />
              );
            }}
          />
        </div>
      )}
    </Screen>
  );
}
