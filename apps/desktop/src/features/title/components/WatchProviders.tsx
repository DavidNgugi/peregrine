import type { WatchAvailabilityDto } from "@peregrine/ipc-client";
import type { ReactElement } from "react";
import { useTranslation } from "react-i18next";

import { imageUrl } from "../../../lib/media.js";

/** The order the groups are drawn in: what is included first, what costs extra after. */
const CATEGORY_ORDER = ["flatrate", "free", "ads", "rent", "buy"] as const;

export interface WatchProvidersProps {
  /** The cached answer, or `null` when there is nothing to draw. */
  readonly data: WatchAvailabilityDto | null;
}

/**
 * Where to watch, as a wall of equal service marks, in the hero's column under its actions.
 *
 * **The marks are the answer, so they are all one size.** A row of differently sized logos reads
 * as a ranking; a fixed 64×36 box gives every service the same weight and lets a wide wordmark and a
 * square mark sit together. The box is light whatever the theme, because several marks are dark
 * artwork on transparency and would disappear on a dark surface.
 *
 * A service without a cached mark falls back to its name in the same box rather than to a broken
 * image or a gap, which is also what keeps the row's rhythm when the provider sent no logo.
 *
 * **A block rather than a section.** It answers the same question the Play button does, so it sits
 * beside that button instead of below the whole hero; the page passes it into `TitleHero`'s slot and
 * the heading steps down from the page's sections to a third level.
 */
export function WatchProviders({ data }: WatchProvidersProps): ReactElement | null {
  const { t } = useTranslation();
  if (data === null || data.fetchedAt === null) {
    return null;
  }

  const groups = CATEGORY_ORDER.map((category) => ({
    category,
    offers: data.offers.filter((offer) => offer.category === category),
  })).filter((group) => group.offers.length > 0);

  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
        <h3 className="text-body font-medium text-text-primary">{t("title.watch.title")}</h3>
        <span className="text-caption text-text-tertiary">
          {t("title.watch.region", { region: data.region })}
        </span>
      </div>
      {groups.length === 0 ? (
        <p className="text-caption text-text-secondary">
          {t("title.watch.empty", { region: data.region })}
        </p>
      ) : (
        <div className="flex flex-col gap-3">
          {groups.map((group) => (
            <div className="flex flex-col gap-1.5" key={group.category}>
              <h4 className="text-caption font-medium tracking-wide text-text-tertiary uppercase">
                {t(`title.watch.category.${group.category}`)}
              </h4>
              <ul className="flex flex-wrap gap-2">
                {group.offers.map((offer) => (
                  <li key={`${offer.category}-${String(offer.providerId)}`}>
                    <span
                      className="flex h-9 w-16 items-center justify-center rounded-md border border-border-subtle bg-white p-1"
                      title={offer.provider}
                    >
                      {offer.logo === null ? (
                        <span className="line-clamp-2 text-center text-[10px] leading-tight text-black">
                          {offer.provider}
                        </span>
                      ) : (
                        <img
                          alt={offer.provider}
                          className="h-full w-full object-contain"
                          src={imageUrl(offer.logo) ?? undefined}
                        />
                      )}
                    </span>
                  </li>
                ))}
              </ul>
            </div>
          ))}
          <p className="text-caption text-text-tertiary">{t("title.watch.attribution")}</p>
        </div>
      )}
    </div>
  );
}
