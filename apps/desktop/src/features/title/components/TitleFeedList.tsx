/**
 * The endless feed at the foot of a title's page: one list, one scroll area, no end until the provider
 * runs out.
 *
 * **It scrolls with the page rather than inside itself, and that is the point.** A rail is a horizontal
 * scroller and a shelf is a bounded vertical one, so a page carrying both is a page with several scroll
 * areas competing for one gesture — which is what a reader objected to. This draws into the page's own
 * scroll and asks for more when the bottom of the list comes near, so there is exactly one thing to
 * scroll and it never stops early.
 *
 * **The trigger is an observer, not arithmetic.** The first version of the grid decided "the reader has
 * reached the end" from a computed row window, which meant a page that did not fill the viewport never
 * asked again until the reader scrolled away and back. An `IntersectionObserver` on a sentinel answers
 * the question directly and keeps answering it as the content grows, so a short first page simply asks
 * for the next one.
 *
 * **The tiles are not virtualized.** The page's own scroller is what `packages/ui`'s `Grid` cannot be
 * used for, and a windowed list needs its own scroll container. What bounds the cost instead is the feed
 * itself: a reader sees the titles of the genres they asked about, and the count grows by one provider
 * page at a time. A reader who scrolls for a very long time is holding a large number of tiles, which is
 * the honest cost of one scroll area and is recorded as owed rather than hidden.
 */

import { EmptyState, TitleCard } from "@peregrine/ui";
import type { RailItemDto } from "@peregrine/ipc-client";
import { useEffect, useRef, type ReactElement } from "react";
import { useTranslation } from "react-i18next";

import { routePath, type Route } from "../../../app/routes.js";
import { downloadCardProps, type DownloadMarks } from "../../../lib/download-status.js";
import { useDownloadProgress } from "../../../lib/download-progress.js";
import { imageUrl } from "../../../lib/media.js";
import type { TitleFeed } from "../hooks/useTitleFeed.js";

/**
 * How near the bottom the sentinel has to be before the next page is asked for.
 *
 * Six hundred pixels is about two rows of tiles: enough that the reader does not meet the end of the
 * list, small enough that a page is not fetched for someone who never scrolls.
 */
const PREFETCH_MARGIN = "600px 0px";

/** Asks for the next page when it comes into view. */
function FeedSentinel({ onReached }: { readonly onReached: () => void }): ReactElement {
  const marker = useRef<HTMLDivElement | null>(null);
  // Read through a ref so a caller's inline arrow does not re-create the observer on every render.
  const reached = useRef(onReached);
  reached.current = onReached;

  useEffect(() => {
    const element = marker.current;
    if (element === null) {
      return;
    }
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) {
          reached.current();
        }
      },
      { rootMargin: PREFETCH_MARGIN },
    );
    observer.observe(element);
    return () => {
      observer.disconnect();
    };
  }, []);

  return <div aria-hidden="true" className="h-px w-full" ref={marker} />;
}

export interface TitleFeedListProps {
  readonly feed: TitleFeed;
  readonly onNavigate: (route: Route) => void;
  /** A download status per title, so a tile wears the mark of a transfer in flight. */
  readonly downloads?: DownloadMarks | undefined;
}

/** Nothing downloading, for a caller that draws the feed without a queue. */
const NO_DOWNLOADS: DownloadMarks = new Map();

/** One title's feed, drawn into whatever scrolls it. */
export function TitleFeedList({
  feed,
  onNavigate,
  downloads,
}: TitleFeedListProps): ReactElement | null {
  const { t } = useTranslation();
  const marks = downloads ?? NO_DOWNLOADS;
  const progress = useDownloadProgress();

  if (feed.items.length === 0) {
    // Nothing to draw and nothing wrong: a title no provider can be asked about has no feed, and the
    // page above is still the answer to the question the reader asked.
    return feed.failed === null ? null : (
      <EmptyState body={t("title.feed.failed.body")} title={t("title.feed.failed.title")} />
    );
  }

  const meta = (item: RailItemDto): string => {
    const word = item.kind === "movie" ? t("discover.kind.movie") : t("discover.kind.show");
    return item.year === null ? word : `${String(item.year)} · ${word}`;
  };
  const routeFor = (item: RailItemDto): Route => ({
    id: item.titleId,
    kind: "title",
    titleKind: item.kind,
  });

  return (
    <section aria-labelledby="title-feed-heading" className="flex flex-col gap-3">
      <div className="flex flex-col gap-1">
        <h2 className="text-title-2 text-text-primary" id="title-feed-heading">
          {t("title.feed.title")}
        </h2>
        <p className="text-caption text-text-tertiary">{t("title.feed.description")}</p>
      </div>

      {/* A plain responsive grid rather than the virtualized `Grid`: its tiles are the page's, so the
          page's scroller is the only one on screen. `auto-fill` at the `lg` tile's width is what makes
          it reflow with the window, which is the one thing the virtualized grid did here. */}
      <ul
        className="grid list-none gap-3"
        style={{ gridTemplateColumns: `repeat(auto-fill, ${String(176)}px)` }}
      >
        {feed.items.map((item) => (
          <li key={item.titleId}>
            <TitleCard
              {...downloadCardProps(marks, item.titleId, t, progress)}
              detailsLabel={t("card.details")}
              href={routePath(routeFor(item))}
              meta={meta(item)}
              onNavigate={() => {
                onNavigate(routeFor(item));
              }}
              overview={item.overview}
              poster={imageUrl(item.poster)}
              rating={item.rating}
              ratingLabel={item.rating === null ? null : t("card.rating", { score: item.rating })}
              size="lg"
              title={item.name}
            />
          </li>
        ))}
      </ul>

      {feed.exhausted ? (
        // The end, said rather than left to inference: a list that simply stops is indistinguishable
        // from one that is still loading.
        <p className="text-caption text-text-tertiary" role="status">
          {feed.failed === null ? t("title.feed.end") : t("title.feed.failed.body")}
        </p>
      ) : (
        <FeedSentinel onReached={feed.loadMore} />
      )}
    </section>
  );
}
