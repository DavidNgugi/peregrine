/**
 * The other qualities a search found, offered as a choice rather than a dead end.
 *
 * A preferred quality that is not there is not a failure: the core keeps the releases it did find
 * and refuses them only for the preference, so this turns that into "here is what is available" with
 * a Download on each. A row carries ids and metadata only — the release's source stays in the core
 * (R11) — and nothing here ranks or filters, because the core already ordered the list.
 */

import { Button, Section } from "@peregrine/ui";
import type { ReactElement } from "react";
import { useTranslation } from "react-i18next";
import { messageOf } from "../../../lib/ipc-message.js";
import type { Route } from "../../../app/routes.js";
import { useReleaseOffers } from "../hooks/useReleaseOffers.js";

interface ReleaseOffersProps {
  /** The title whose alternatives these are. */
  readonly titleId: number;
  /** Changes when the acquire job settles, because that is when a search persists offers. */
  readonly reloadKey: string | null;
  readonly onNavigate: (route: Route) => void;
}

/** What `useTranslation` hands a component. */
type Translate = ReturnType<typeof useTranslation>["t"];

/** Bytes as a row shows them, matching the title page's own table. */
function formatSize(bytes: number | null): string | null {
  if (bytes === null) {
    return null;
  }
  if (bytes === 0) {
    return "0 B";
  }
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let index = 0;
  while (value >= 1024 && index < units.length - 1) {
    value /= 1024;
    index += 1;
  }
  const rounded = value < 10 ? Math.round(value * 10) / 10 : Math.round(value);
  return `${String(rounded)} ${units[index] ?? "B"}`;
}

/**
 * Why the preference excluded a release, in words rather than the domain's own spelling.
 *
 * A reason carries its value after a colon (`resolution-not-allowed:2160p`), so the prefix names
 * the rule and the suffix is what to show — and an unknown prefix is printed as it came rather than
 * swallowed, because a reason nobody translated is still a reason.
 */
function reasonLabel(reason: string, t: Translate): string {
  const separator = reason.indexOf(":");
  if (separator > 0) {
    const kind = reason.slice(0, separator);
    const value = reason.slice(separator + 1);
    if (kind === "resolution-not-allowed") {
      return t("title.offers.reason.resolution", { value });
    }
    if (kind === "source-not-allowed") {
      return t("title.offers.reason.source", { value });
    }
  }
  return reason;
}

/** The alternatives picker. Draws nothing when the core kept none. */
export function ReleaseOffers({
  titleId,
  reloadKey,
  onNavigate,
}: ReleaseOffersProps): ReactElement | null {
  const { t } = useTranslation();
  const offers = useReleaseOffers(titleId, reloadKey);

  if (offers.loading || offers.failed || offers.offers.length === 0) {
    return null;
  }

  return (
    <Section description={t("title.offers.description")} title={t("title.offers.title")}>
      <div className="flex flex-col gap-3">
        <ul className="flex list-none flex-col gap-2">
          {offers.offers.map((offer) => {
            const size = formatSize(offer.sizeBytes);
            const seeders =
              offer.seeders === null
                ? t("title.offers.noSeeders")
                : t("title.offers.seeders", { seeders: offer.seeders });
            const protocol = offer.protocol === "usenet" ? t("downloads.protocol.usenet") : null;
            const facts = [offer.indexer, protocol, size, seeders, reasonLabel(offer.reason, t)]
              .filter((part): part is string => part !== null && part !== "")
              .join(" · ");
            return (
              <li className="flex items-center justify-between gap-4" key={offer.id}>
                <div className="flex min-w-0 flex-col gap-1">
                  <span className="truncate text-body text-text-primary">{offer.releaseTitle}</span>
                  <span className="text-caption text-text-tertiary">{facts}</span>
                </div>
                <div className="flex shrink-0 items-center gap-2">
                  <Button
                    disabled={offers.busyId !== null}
                    onClick={() => offers.choose(offer.id)}
                    size="sm"
                    variant="primary"
                  >
                    {t("title.offers.download")}
                  </Button>
                  <Button
                    disabled={offers.busyId !== null}
                    onClick={() => offers.dismiss(offer.id)}
                    size="sm"
                    variant="ghost"
                  >
                    {t("title.offers.dismiss")}
                  </Button>
                </div>
              </li>
            );
          })}
        </ul>
        {offers.started ? (
          <div className="flex items-center gap-3" role="status">
            <p className="text-caption text-text-secondary">{t("title.offers.queued")}</p>
            <Button
              onClick={() => {
                onNavigate({ kind: "downloads" });
              }}
              size="sm"
              variant="secondary"
            >
              {t("downloads.view")}
            </Button>
          </div>
        ) : null}
        {offers.error === null ? null : (
          <p className="text-caption text-state-danger" role="status">
            {messageOf(offers.error)}
          </p>
        )}
      </div>
    </Section>
  );
}
