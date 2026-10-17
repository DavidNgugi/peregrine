import { useEffect, useRef, type ReactElement } from "react";
import { useTranslation } from "react-i18next";

import type { Profile } from "./profile.js";

export interface ProfileSwitcherProps {
  readonly open: boolean;
  readonly profile: Profile;
  /** The featured title's backdrop, already resolved through the asset protocol. */
  readonly backdrop: string | null;
  /** Poster URLs, used only when no backdrop has been reported yet. */
  readonly posters: readonly string[];
  readonly onClose: () => void;
}

/**
 * The "who's watching" screen.
 *
 * The shape follows a television selector: one full-window backdrop, the avatars stacked over its
 * left edge, and the rail bleeding into the picture rather than sitting in a panel of its own. The
 * gradient is what separates the labels from the artwork, so the image stays whole.
 *
 * One profile exists until the account backend lands, and the Add tile says so rather than opening
 * a form that cannot save.
 */
export function ProfileSwitcher({
  open,
  profile,
  backdrop,
  posters,
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

  const image = backdrop ?? posters[0] ?? null;

  return (
    <div
      aria-label={t("profile.who")}
      aria-modal="true"
      className="fixed inset-0 z-50 overflow-hidden bg-black text-white"
      role="dialog"
    >
      {image === null ? (
        <div className="absolute inset-0 bg-[radial-gradient(circle_at_70%_30%,hsl(212_60%_22%),hsl(220_40%_5%))]" />
      ) : (
        <img alt="" className="absolute inset-0 size-full object-cover" src={image} />
      )}
      <div className="absolute inset-0 bg-gradient-to-r from-black/95 via-black/55 to-transparent" />
      <div className="absolute inset-0 bg-gradient-to-t from-black/85 via-transparent to-black/25" />
      <div className="relative flex h-full">
        <aside className="flex w-[230px] shrink-0 flex-col gap-5 px-8 py-10">
          <p className="text-sm font-semibold tracking-tight text-white/90">Peregrine</p>
          <h1 className="text-xs font-medium uppercase tracking-[0.22em] text-white/55">
            {t("profile.who")}
          </h1>
          <ul className="mt-2 flex list-none flex-col gap-5 p-0">
            <li className="flex items-center gap-4">
              <span
                aria-hidden="true"
                className="grid size-16 shrink-0 place-items-center rounded-md text-2xl font-semibold ring-2 ring-white"
                style={{ backgroundColor: `hsl(${String(profile.hue)} 52% 42%)` }}
              >
                {profile.name.trim().slice(0, 1).toUpperCase() || "?"}
              </span>
              <span className="text-xl font-medium text-white drop-shadow">{profile.name}</span>
            </li>
            <li className="flex items-center gap-4">
              <button
                aria-label={t("profile.add")}
                className="grid size-16 shrink-0 cursor-not-allowed place-items-center rounded-md border border-white/25 bg-black/20 text-2xl text-white/35"
                disabled
                title={t("profile.local")}
                type="button"
              >
                +
              </button>
              <span className="max-w-[6.5rem] text-[11px] leading-snug text-white/45">
                {t("profile.local")}
              </span>
            </li>
          </ul>
          <button
            className="mt-auto self-start rounded-md border border-white/25 bg-black/35 px-5 py-2 text-sm text-white/85 outline-none backdrop-blur-sm transition-colors hover:border-white/60 hover:text-white focus-visible:ring-2 focus-visible:ring-white"
            onClick={onClose}
            ref={done}
            type="button"
          >
            {t("profile.done")}
          </button>
        </aside>
        <div className="min-w-0 flex-1" />
      </div>
    </div>
  );
}
