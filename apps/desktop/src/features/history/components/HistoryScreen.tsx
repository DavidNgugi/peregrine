import { Button, EmptyState, ErrorState, Panel, Screen, SettingsRow } from "@peregrine/ui";
import { useState, type ReactElement } from "react";
import { useTranslation } from "react-i18next";

import { useWatchHistory, type WatchHistoryView } from "../hooks/useWatchHistory.js";

/** How many rows one page draws before a reader asks for more. */
export const HISTORY_PAGE = 25;

/** The list itself, separate from the read so a test can hold thirty rows without IPC. */
export function HistoryList({ history }: { readonly history: WatchHistoryView }): ReactElement {
  const { t } = useTranslation();
  const [visible, setVisible] = useState(HISTORY_PAGE);
  const shown = history.rows.slice(0, visible);

  return (
    <Panel className="flex flex-col divide-y divide-border-subtle overflow-hidden">
      {history.loading ? null : history.failed !== null ? (
        <ErrorState body={t("settings.history.failed")} title={t("history.title")} />
      ) : history.rows.length === 0 ? (
        <EmptyState title={t("settings.history.empty")} />
      ) : (
        shown.map((row) => {
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
      {history.rows.length > HISTORY_PAGE ? (
        <div className="flex items-center justify-between gap-3 px-3 py-2">
          <span className="text-caption text-text-tertiary">
            {t("settings.history.showing", {
              shown: Math.min(visible, history.rows.length),
              total: history.rows.length,
            })}
          </span>
          <Button
            onClick={() => {
              setVisible((current) =>
                current < history.rows.length ? current + HISTORY_PAGE : HISTORY_PAGE,
              );
            }}
            size="sm"
            variant="ghost"
          >
            {visible < history.rows.length
              ? t("settings.history.more")
              : t("settings.history.less")}
          </Button>
        </div>
      ) : null}
    </Panel>
  );
}

/** What this device has watched, newest first, with the episode and the date it happened. */
export function HistoryScreen(): ReactElement {
  const { t } = useTranslation();
  const history = useWatchHistory();

  return (
    <Screen subtitle={t("history.subtitle")} title={t("history.title")}>
      <HistoryList history={history} />
    </Screen>
  );
}
