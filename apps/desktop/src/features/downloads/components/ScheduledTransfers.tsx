import { Button } from "@peregrine/ui";
import { useTranslation } from "react-i18next";
import type { ReactElement } from "react";

import { useSchedules } from "../hooks/useSchedules.js";

/**
 * The downloads waiting for their date.
 *
 * Hidden when empty, because an empty heading over nothing is a promise. Cancelling deletes the
 * schedule; a job it already started is the Transfer queue's to cancel, which is why the row's
 * state says whether it is still waiting.
 */
export function ScheduledTransfers(): ReactElement | null {
  const { t } = useTranslation();
  const schedules = useSchedules();
  if (schedules.rows.length === 0 && schedules.failed === null) return null;

  return (
    <section className="flex flex-col gap-2">
      <h2 className="text-title text-text-primary">{t("downloads.scheduled.title")}</h2>
      <p className="text-caption text-text-secondary">{t("downloads.scheduled.description")}</p>
      {schedules.failed === null ? null : (
        <p className="text-caption text-state-danger" role="status">
          {t("downloads.scheduled.failed")}
        </p>
      )}
      <ul className="flex list-none flex-col gap-1 p-0">
        {schedules.rows.map((row) => {
          const episode =
            row.seasonNumber !== null && row.episodeNumber !== null
              ? t("downloads.episode", {
                  season: row.seasonNumber,
                  episode: String(row.episodeNumber).padStart(2, "0"),
                })
              : null;
          return (
            <li
              className="flex flex-wrap items-center gap-3 rounded-md border border-border-subtle bg-surface-1 px-3 py-2"
              key={row.id}
            >
              <span className="min-w-0 truncate text-body text-text-primary">
                {row.title ?? t("downloads.scheduled.unknown")}
              </span>
              {episode === null ? null : (
                <span className="text-caption text-text-tertiary">{episode}</span>
              )}
              <span className="text-caption text-text-secondary">
                {t("downloads.scheduled.from", { date: row.releaseOn })}
              </span>
              <span className="text-caption text-text-tertiary">
                {t("downloads.scheduled.state." + row.state)}
              </span>
              {row.error === null ? null : (
                <span className="text-caption text-state-danger">{row.error}</span>
              )}
              <span className="ms-auto">
                <Button
                  onClick={() => {
                    schedules.cancel(row.id);
                  }}
                  size="sm"
                  variant="secondary"
                >
                  {t("downloads.scheduled.cancel")}
                </Button>
              </span>
            </li>
          );
        })}
      </ul>
    </section>
  );
}
