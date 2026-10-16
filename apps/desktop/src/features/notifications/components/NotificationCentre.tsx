/**
 * The notification centre's bell: a count, and the rows behind it.
 *
 * It exists because a centre row written and never shown is not a notification. The panel is
 * read-only for now: the rows carry title and body, and the dismiss/read actions are the centre
 * repository's, which no command exposes yet.
 */

import { Button } from "@peregrine/ui";
import { useCallback, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNotifications } from "../hooks/useNotifications.js";

export function NotificationCentre() {
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
              {notifications.rows.map((row) => (
                <li className="rounded-sm px-1 py-1" key={row.id}>
                  <p className="text-caption text-text-primary">{row.title}</p>
                  {row.body === null ? null : (
                    <p className="text-caption text-text-secondary">{row.body}</p>
                  )}
                </li>
              ))}
            </ul>
          )}
        </div>
      ) : null}
    </div>
  );
}
