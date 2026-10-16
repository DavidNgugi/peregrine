/**
 * The notification centre's bell: a count, and the rows behind it.
 *
 * It exists because a centre row written and never shown is not a notification. A row whose
 * event named a title opens that title: the core sends the title's kind and id because the stored
 * deep link carries no kind, and a guessed kind would open the wrong screen. The dismiss and read
 * actions still belong to the centre repository, which exposes no command for either yet.
 */

import { Button } from "@peregrine/ui";
import { useCallback, useState } from "react";
import { useTranslation } from "react-i18next";
import type { Route, TitleKind } from "../../../app/routes.js";
import { useNotifications } from "../hooks/useNotifications.js";

/** The bell's dependencies: the one thing a row can do is open its title. */
interface NotificationCentreProps {
  readonly onNavigate: (route: Route) => void;
}

/** Narrowing rather than a cast: a kind this build does not know must not become a route. */
function isTitleKind(value: string | null): value is TitleKind {
  return value === "movie" || value === "show";
}

export function NotificationCentre({ onNavigate }: NotificationCentreProps) {
  const { t } = useTranslation();
  const notifications = useNotifications();
  const [open, setOpen] = useState(false);

  const toggle = useCallback(() => {
    setOpen((current) => {
      // Opening re-reads, so the panel never shows a stale list after an event was recorded.
      if (!current) {
        void notifications.refresh();
      }
      return !current;
    });
  }, [notifications]);

  const unread = notifications.rows.filter((row) => !row.read).length;

  const openTitle = useCallback(
    (kind: string | null, subjectId: number | null): void => {
      if (!isTitleKind(kind) || subjectId === null) {
        return;
      }
      onNavigate({ kind: "title", titleKind: kind, id: subjectId });
      setOpen(false);
    },
    [onNavigate],
  );

  return (
    <div className="relative">
      <Button
        aria-expanded={open}
        onClick={toggle}
        size="sm"
        variant="ghost"
      >
        {unread > 0 ? t("shell.notifications.labelWithCount", { count: unread }) : t("shell.notifications.label")}
      </Button>
      {open ? (
        <div
          className="absolute end-2 top-full z-20 mt-2 w-80 rounded-md border border-border-subtle bg-surface-1 p-2 shadow-lg"
          role="region"
        >
          <p className="px-1 pb-1 text-caption text-text-secondary">{t("shell.notifications.title")}</p>
          {notifications.failed ? (
            <p className="px-1 py-2 text-caption text-state-danger">{t("shell.notifications.failed")}</p>
          ) : notifications.rows.length === 0 ? (
            <p className="px-1 py-2 text-caption text-text-secondary">{t("shell.notifications.empty")}</p>
          ) : (
            <ul className="flex max-h-80 flex-col gap-1 overflow-y-auto">
              {notifications.rows.map((row) => {
                const opens = isTitleKind(row.titleKind) && row.subjectId !== null;
                return (
                  <li className="rounded-sm" key={row.id}>
                    <button
                      className="flex w-full flex-col items-start rounded-sm px-1 py-1 text-start enabled:hover:bg-surface-2 disabled:cursor-default"
                      disabled={!opens}
                      onClick={() => {
                        openTitle(row.titleKind, row.subjectId);
                      }}
                      type="button"
                    >
                      <p className="text-caption text-text-primary">{row.title}</p>
                      {row.body === null ? null : (
                        <p className="text-caption text-text-secondary">{row.body}</p>
                      )}
                    </button>
                  </li>
                );
              })}
            </ul>
          )}
        </div>
      ) : null}
    </div>
  );
}
