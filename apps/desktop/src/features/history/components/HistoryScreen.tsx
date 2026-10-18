import { Button, Dialog, EmptyState, ErrorState, Link, Panel, Screen } from "@peregrine/ui";
import { useState, type ReactElement } from "react";
import { useTranslation } from "react-i18next";

import { routePath, type Route } from "../../../app/routes.js";
import { imageUrl } from "../../../lib/media.js";
import { clearWatchHistory } from "@peregrine/ipc-client";
import { useWatchHistory, type WatchHistoryView } from "../hooks/useWatchHistory.js";

export const HISTORY_PAGE = 25;

export function HistoryList({
  history,
  onNavigate,
  onAskClear,
}: {
  readonly history: WatchHistoryView;
  readonly onNavigate: (route: Route) => void;
  readonly onAskClear: (titleId: number | null, name: string) => void;
}): ReactElement {
  const { t } = useTranslation();
  const [visible, setVisible] = useState(HISTORY_PAGE);
  const shown = history.rows.slice(0, visible);

  return (
    <Panel className="flex flex-col divide-y divide-border-subtle overflow-hidden">
      <div className="flex items-center justify-between gap-3 px-3 py-2">
        <span className="text-caption text-text-tertiary">
          {t("history.count", { count: history.rows.length })}
        </span>
        <Button
          disabled={history.rows.length === 0}
          onClick={() => {
            onAskClear(null, t("history.all"));
          }}
          size="sm"
          variant="ghost"
        >
          {t("history.clearAll")}
        </Button>
      </div>
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
          const poster = imageUrl(row.poster);
          const name = row.title ?? t("settings.history.unknown");
          const body = (
            <span className="flex min-w-0 items-center gap-3">
              <span className="block h-12 w-8 shrink-0 overflow-hidden rounded-sm border border-border-subtle bg-surface-2">
                {poster === null ? null : (
                  <img
                    alt=""
                    aria-hidden="true"
                    className="size-full object-cover"
                    decoding="async"
                    loading="lazy"
                    src={poster}
                  />
                )}
              </span>
              <span className="flex min-w-0 flex-col">
                <span className="truncate text-body text-text-primary">{name}</span>
                {episode === null ? null : (
                  <span className="text-caption text-text-tertiary">{episode}</span>
                )}
              </span>
            </span>
          );
          const route: Route | null =
            row.titleKind === null
              ? null
              : { kind: "title", titleKind: row.titleKind, id: row.titleId };
          return (
            <div
              className="flex w-full items-center justify-between gap-3 px-3 py-2"
              key={`${String(row.titleId)}-${episode ?? "title"}-${String(row.updatedAt)}`}
            >
              {route === null ? (
                body
              ) : (
                <Link
                  href={routePath(route)}
                  onNavigate={() => {
                    onNavigate(route);
                  }}
                >
                  {body}
                </Link>
              )}
              <span className="flex shrink-0 items-center gap-3">
                <span className="text-caption text-text-secondary">
                  {row.watched
                    ? t("settings.history.watched")
                    : t(
                        row.seasonNumber === null
                          ? "history.progressTitle"
                          : "history.progressEpisode",
                        { percent: Math.round((row.percent ?? 0) * 100) },
                      )}
                </span>
                <span className="text-caption text-text-tertiary">
                  {new Date(row.updatedAt).toLocaleDateString()}
                </span>
                <Button
                  onClick={() => {
                    onAskClear(row.titleId, name);
                  }}
                  size="sm"
                  variant="ghost"
                >
                  {t("history.remove")}
                </Button>
              </span>
            </div>
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

export function HistoryScreen({
  onNavigate,
}: {
  readonly onNavigate: (route: Route) => void;
}): ReactElement {
  const { t } = useTranslation();
  const history = useWatchHistory();
  const [confirm, setConfirm] = useState<{ titleId: number | null; name: string } | null>(null);
  const [failed, setFailed] = useState(false);

  const clear = async (): Promise<void> => {
    if (confirm === null) return;
    try {
      await clearWatchHistory(confirm.titleId ?? undefined);
      await history.refresh();
      setConfirm(null);
      setFailed(false);
    } catch {
      setFailed(true);
    }
  };

  return (
    <Screen subtitle={t("history.subtitle")} title={t("history.title")}>
      <HistoryList
        history={history}
        onAskClear={(titleId, name) => {
          setConfirm({ titleId, name });
          setFailed(false);
        }}
        onNavigate={onNavigate}
      />
      {confirm === null ? null : (
        <Dialog
          closeLabel={t("history.cancel")}
          description={t("history.clearBody", { name: confirm.name })}
          onClose={() => {
            setConfirm(null);
          }}
          title={t("history.clearTitle")}
        >
          <div className="flex justify-end gap-2">
            <Button
              onClick={() => {
                setConfirm(null);
              }}
              variant="ghost"
            >
              {t("history.cancel")}
            </Button>
            <Button
              onClick={() => {
                void clear();
              }}
              variant="danger"
            >
              {t("history.confirm")}
            </Button>
          </div>
          {failed ? (
            <p className="text-caption text-state-danger" role="alert">
              {t("history.clearFailed")}
            </p>
          ) : null}
        </Dialog>
      )}
    </Screen>
  );
}
