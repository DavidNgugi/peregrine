import { useEffect, useRef, type ReactElement } from "react";
import { useTranslation } from "react-i18next";

import { ProfileAvatar } from "./ProfileAvatar.js";
import type { Profile } from "./profile.js";

export interface ProfileSwitcherProps {
  readonly open: boolean;
  readonly profile: Profile;
  readonly onClose: () => void;
}

/**
 * The "who's watching" screen, opened from the avatar in the sidebar or the Discover toolbar.
 *
 * The rail is vertical and the backdrop is the content behind it, which is the shape a television
 * selector uses and the one the shell can draw without a second window. One profile exists until
 * the account backend lands; the note says so rather than offering an "Add" that cannot work.
 */
export function ProfileSwitcher({
  open,
  profile,
  onClose,
}: ProfileSwitcherProps): ReactElement | null {
  const { t } = useTranslation();
  const done = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!open) return;
    done.current?.focus();
    const onKey = (event: KeyboardEvent): void => {
      if (event.key === "Escape") {
        event.preventDefault();
        onClose();
      }
    };
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("keydown", onKey);
    };
  }, [open, onClose]);

  if (!open) return null;

  return (
    <div
      aria-label={t("profile.who")}
      aria-modal="true"
      className="fixed inset-0 z-50 flex flex-col bg-surface-0/95 backdrop-blur-sm"
      role="dialog"
    >
      <div className="flex min-h-0 flex-1 items-stretch">
        <div className="flex w-56 shrink-0 flex-col items-center gap-6 border-r border-border-subtle bg-surface-1 px-4 py-10">
          <p className="text-xs font-semibold uppercase tracking-[0.18em] text-text-muted">
            {t("profile.who")}
          </p>
          <ProfileAvatar
            current
            label={t("profile.current", { name: profile.name })}
            onClick={() => {
              done.current?.focus();
            }}
            profile={profile}
            size="md"
          />
          <div className="text-center">
            <p className="text-sm font-medium text-text-primary">{profile.name}</p>
            <p className="mt-1 text-xs text-text-muted">{t("profile.local")}</p>
          </div>
        </div>
        <div className="flex min-w-0 flex-1 flex-col items-center justify-center gap-4 px-10">
          <h1 className="text-3xl font-semibold text-text-primary">{t("profile.who")}</h1>
          <p className="max-w-sm text-center text-sm text-text-muted">{t("profile.note")}</p>
          <button
            className="mt-4 rounded-md border border-border-subtle px-5 py-2 text-sm text-text-primary outline-none hover:bg-surface-2 focus-visible:ring-2 focus-visible:ring-accent"
            onClick={onClose}
            ref={done}
            type="button"
          >
            {t("profile.done")}
          </button>
        </div>
      </div>
    </div>
  );
}
