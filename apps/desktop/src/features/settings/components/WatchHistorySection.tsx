import { EmptyState, ErrorState, SettingsRow, SettingsSection } from "@peregrine/ui";
import { useTranslation } from "react-i18next";
import type { ReactElement } from "react";

import type { WatchHistoryView } from "../hooks/useWatchHistory.js";

/** The account's viewing history, which is the long form of the bars on the season list. */
export function WatchHistorySection({
  history,
}: {
  readonly history: WatchHistoryView;
}): ReactElement {
  const { t } = useTranslation();
  return (
    <SettingsSection
      description={t("settings.history.description")}
      title={t("settings.history.title")}
    >
      {history.loading ? null : history.failed !== null ? (
        <ErrorState body={t("settings.history.failed")} title={t("settings.history.title")} />
      ) : history.rows.length === 0 ? (
        <EmptyState title={t("settings.history.empty")} />
      ) : (
        history.rows.map((row) => {
          const episode =
            row.seasonNumber !== null && row.episodeNumber !== null
              ? `S${String(row.seasonNumber)}E${String(row.episodeNumber).padStart(2, "0")}`
              : null;
          return (
            <SettingsRow
              description={episode ?? undefined}
              key={`${String(row.titleId)}-${episode ?? "title"}-${String(row.updatedAt)}`}
              label={row.title ?? t("settings.history.unknown")}
            >
              <span className="text-caption text-text-secondary">
                {row.watched
                  ? t("settings.history.watched")
                  : t("settings.history.progress", {
                      percent: Math.round((row.percent ?? 0) * 100),
                    })}
              </span>
              <span className="text-caption text-text-tertiary">
                {new Date(row.updatedAt).toLocaleDateString()}
              </span>
            </SettingsRow>
          );
        })
      )}
    </SettingsSection>
  );
}
