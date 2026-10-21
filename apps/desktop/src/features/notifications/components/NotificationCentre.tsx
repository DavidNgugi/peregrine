/**
 * The notification centre's bell: a count, and the rows behind it.
 *
 * It exists because a centre row written and never shown is not a notification. A row whose
 * event named a title opens that title: the core sends the title's kind and id because the stored
 * deep link carries no kind, and a guessed kind would open the wrong screen. Opening a row marks
 * it read, a row can be dismissed, and the whole list can be marked read.
 *
 * The panel is portalled to the body and placed from the bell's rect. The bell lives in the
 * sidebar header, whose width is often narrower than the list; an absolutely positioned panel
 * there is clipped by the sidebar's edge on the left. Placement clamps the panel to the viewport,
 * and a narrow window gets a narrower panel rather than one whose rows start off-screen.
 */

import type { NotificationDto } from "@peregrine/ipc-client";
import { Button, IconButton } from "@peregrine/ui";
import { useCallback, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import type { Route, TitleKind } from "../../../app/routes.js";
import { useNotifications } from "../hooks/useNotifications.js";
import { panelPlacement, type PanelPlacement } from "./notification-placement.js";

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
  const [placement, setPlacement] = useState<PanelPlacement | null>(null);
  const anchor = useRef<HTMLDivElement | null>(null);

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

  // Opening a row is reading it: the flag lives in the repository, so the mutation is what
  // clears the count, not the navigation.
  const openRow = useCallback(
    (row: NotificationDto): void => {
      void notifications.markRead(row.id);
      if (!isTitleKind(row.titleKind) || row.subjectId === null) {
        return;
      }
      onNavigate({ kind: "title", titleKind: row.titleKind, id: row.subjectId });
      setOpen(false);
    },
    [notifications, onNavigate],
  );

  useLayoutEffect(() => {
    if (!open) {
      setPlacement(null);
      return;
    }
    const place = (): void => {
      setPlacement(
        panelPlacement(
          anchor.current?.getBoundingClientRect() ?? null,
          window.innerWidth,
          window.innerHeight,
        ),
      );
    };
    place();
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, true);
    return () => {
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", place, true);
    };
  }, [open]);

  const label =
    unread > 0
      ? t("shell.notifications.labelWithCount", { count: unread })
      : t("shell.notifications.label");

  const panel =
    open && placement !== null
      ? createPortal(
          <div
            aria-label={t("shell.notifications.title")}
            className="fixed z-50 flex flex-col overflow-hidden rounded-md border border-border-subtle bg-surface-1 p-2 shadow-lg"
            role="region"
            style={{
              top: placement.top,
              left: placement.left,
              width: placement.width,
              maxHeight: placement.maxHeight,
            }}
          >
            <div className="flex shrink-0 items-center justify-between px-1 pb-1">
              <p className="text-caption text-text-secondary">{t("shell.notifications.title")}</p>
              {unread > 0 ? (
                <Button onClick={() => notifications.markAllRead()} size="sm" variant="ghost">
                  {t("shell.notifications.markAll")}
                </Button>
              ) : null}
            </div>
            {notifications.failed ? (
              <p className="px-1 py-2 text-caption text-state-danger">
                {t("shell.notifications.failed")}
              </p>
            ) : notifications.rows.length === 0 ? (
              <p className="px-1 py-2 text-caption text-text-secondary">
                {t("shell.notifications.empty")}
              </p>
            ) : (
              <ul className="flex min-h-0 flex-1 flex-col gap-1 overflow-y-auto">
                {notifications.rows.map((row) => (
                  <li className="flex items-start gap-1 rounded-sm" key={row.id}>
                    <button
                      className="flex min-w-0 flex-1 flex-col items-start rounded-sm px-1 py-1 text-start hover:bg-surface-2"
                      onClick={() => {
                        openRow(row);
                      }}
                      type="button"
                    >
                      <p className="text-caption text-text-primary">
                        {row.read ? row.title : `• ${row.title}`}
                      </p>
                      {row.body === null ? null : (
                        <p className="text-caption text-text-secondary">{row.body}</p>
                      )}
                    </button>
                    <Button onClick={() => notifications.dismiss(row.id)} size="sm" variant="ghost">
                      {t("shell.notifications.dismiss")}
                    </Button>
                  </li>
                ))}
              </ul>
            )}
          </div>,
          document.body,
        )
      : null;

  return (
    <div className="relative" ref={anchor}>
      <div className="relative">
        <IconButton
          aria-expanded={open}
          icon="bell"
          label={label}
          onClick={toggle}
          size="md"
          title={label}
        />
        {unread > 0 ? (
          <span
            aria-hidden="true"
            className="pointer-events-none absolute -end-1 -top-1 grid min-w-4 place-items-center rounded-full bg-accent px-1 text-[10px] font-semibold leading-4 text-text-on-accent"
          >
            {unread > 9 ? "9+" : unread}
          </span>
        ) : null}
      </div>
      {panel}
    </div>
  );
}
