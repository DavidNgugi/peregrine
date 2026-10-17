import { useEffect, useRef, type ReactElement } from "react";
import { useTranslation } from "react-i18next";

import type { Profile } from "./profile.js";

export interface ProfileSwitcherProps {
  readonly open: boolean;
  readonly profile: Profile;
  /** Poster image URLs, already resolved through the asset protocol. */
  readonly posters: readonly string[];
  readonly onClose: () => void;
}

/**
 * The "who's watching" screen.
 *
 * The shape is the television selector: a dark full-window surface, the avatars stacked on the left
 * with the focused one enlarged and ringed, and a wall of rolling posters behind them. The rail is
 * the only interactive column, so the eye has one place to go; the posters are decoration and are
 * not focusable.
 *
 * One profile exists until the account backend lands, and the Add tile says that rather than
 * opening a form that cannot save.
 */
export function ProfileSwitcher({
  open,
  profile,
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

  return (
    <div
      aria-label={t("profile.who")}
      aria-modal="true"
      className="fixed inset-0 z-50 overflow-hidden bg-black text-white"
      role="dialog"
    >
      <PosterWall posters={posters} />
      <div className="absolute inset-0 bg-gradient-to-r from-black via-black/80 to-black/10" />
      <div className="relative flex h-full">
        <aside className="flex w-[320px] flex-col gap-5 px-10 py-12">
          <p className="text-base font-semibold tracking-tight text-white">Peregrine</p>
          <h1 className="text-sm font-medium uppercase tracking-[0.22em] text-white/55">
            {t("profile.who")}
          </h1>
          <ul className="mt-2 flex list-none flex-col gap-6 p-0">
            <li className="flex items-center gap-5">
              <span
                aria-hidden="true"
                className="grid size-20 place-items-center rounded-md text-2xl font-semibold ring-2 ring-white"
                style={{ backgroundColor: `hsl(${String(profile.hue)} 52% 42%)` }}
              >
                {profile.name.trim().slice(0, 1).toUpperCase() || "?"}
              </span>
              <span className="text-2xl font-medium text-white">{profile.name}</span>
            </li>
            <li className="flex items-center gap-5">
              <button
                aria-label={t("profile.add")}
                className="grid size-20 cursor-not-allowed place-items-center rounded-md border border-white/20 text-3xl text-white/35"
                disabled
                title={t("profile.local")}
                type="button"
              >
                +
              </button>
              <span className="max-w-[8rem] text-xs leading-snug text-white/45">
                {t("profile.local")}
              </span>
            </li>
          </ul>
          <div className="mt-auto">
            <button
              className="rounded-md border border-white/25 px-5 py-2 text-sm text-white/85 outline-none transition-colors hover:border-white/50 hover:text-white focus-visible:ring-2 focus-visible:ring-white"
              onClick={onClose}
              ref={done}
              type="button"
            >
              {t("profile.done")}
            </button>
          </div>
        </aside>
        <div className="min-w-0 flex-1" />
      </div>
    </div>
  );
}

/**
 * Two columns of posters drifting in opposite directions.
 *
 * Each column carries its list twice and translates by half its height, so the loop has no seam.
 * The reduced-motion block in `styles.css` freezes both columns for a reader who asks for that.
 */
function PosterWall({ posters }: { readonly posters: readonly string[] }): ReactElement {
  if (posters.length === 0) {
    return (
      <div
        aria-hidden="true"
        className="absolute inset-0 bg-[radial-gradient(circle_at_70%_30%,hsl(212_60%_22%),hsl(220_40%_6%))]"
      />
    );
  }
  const half = Math.ceil(posters.length / 2);
  return (
    <div
      aria-hidden="true"
      className="absolute inset-y-0 right-0 flex w-[72%] gap-3 overflow-hidden opacity-55"
    >
      <PosterColumn posters={posters.slice(0, half)} direction="up" />
      <PosterColumn posters={posters.slice(half)} direction="down" />
    </div>
  );
}

function PosterColumn({
  posters,
  direction,
}: {
  readonly posters: readonly string[];
  readonly direction: "up" | "down";
}): ReactElement {
  const doubled = [...posters, ...posters];
  return (
    <div className="w-1/2">
      <div
        className={`flex flex-col gap-3 ${direction === "up" ? "profile-posters-up" : "profile-posters-down"}`}
      >
        {doubled.map((poster, index) => (
          <img
            alt=""
            className="aspect-[2/3] w-full rounded-sm object-cover"
            key={`${poster}-${String(index)}`}
            loading="lazy"
            src={poster}
          />
        ))}
      </div>
    </div>
  );
}
