import { IconButton } from "@peregrine/ui";
import { useEffect, useRef, useState, type ReactElement } from "react";
import { useTranslation } from "react-i18next";

import { ProfileEditor } from "./ProfileEditor.js";
import { ProfileImage } from "./ProfileImage.js";
import type { Avatar, Profile } from "./profile.js";

export interface ProfileSwitcherProps {
  readonly open: boolean;
  readonly profiles: readonly Profile[];
  readonly activeId: string;
  /** Featured backdrops, already resolved through the asset protocol; rotated in the background. */
  readonly backdrops: readonly string[];
  /** Poster URLs, used only when no backdrop has been reported yet. */
  readonly posters: readonly string[];
  readonly onSelect: (id: string) => void;
  readonly onAdd: (name: string, avatar: Avatar) => void;
  readonly onRename: (id: string, name: string, avatar: Avatar) => void;
  readonly onClose: () => void;
}

type Editing = { readonly mode: "create" } | { readonly mode: "edit"; readonly id: string };

/**
 * The "who's watching" screen.
 *
 * A television selector: one full-window backdrop that crossfades between the featured titles, a
 * narrow avatar rail bleeding over its left edge, the focused profile ringed with its name beside
 * it, and a pencil that opens the editor. Choosing an avatar enters that profile; choosing is the
 * exit.
 */
export function ProfileSwitcher({
  open,
  profiles,
  activeId,
  backdrops,
  posters,
  onSelect,
  onAdd,
  onRename,
  onClose,
}: ProfileSwitcherProps): ReactElement | null {
  const { t } = useTranslation();
  const [index, setIndex] = useState(0);
  const [editing, setEditing] = useState<Editing | null>(null);
  const avatarRefs = useRef(new Map<string, HTMLButtonElement>());

  const images = backdrops.length > 0 ? backdrops : posters.length > 0 ? [posters[0]!] : [];

  useEffect(() => {
    if (!open) {
      setEditing(null);
      setIndex(0);
      return;
    }
    avatarRefs.current.get(activeId)?.focus();
  }, [activeId, open]);

  useEffect(() => {
    if (!open || images.length < 2) return;
    const timer = setInterval(() => {
      setIndex((current) => (current + 1) % images.length);
    }, 7000);
    return () => {
      clearInterval(timer);
    };
  }, [images.length, open]);

  useEffect(() => {
    if (!open) return;
    const onKey = (event: KeyboardEvent): void => {
      if (event.key !== "Escape" || editing !== null) return;
      event.preventDefault();
      onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("keydown", onKey);
    };
  }, [editing, onClose, open]);

  if (!open) return null;

  const edited = editing?.mode === "edit" ? profiles.find((p) => p.id === editing.id) : undefined;

  return (
    <div
      aria-label={t("profile.who")}
      aria-modal="true"
      className="fixed inset-0 z-50 overflow-hidden bg-black text-white"
      role="dialog"
    >
      {images.length === 0 ? (
        <div className="absolute inset-0 bg-[radial-gradient(circle_at_70%_30%,hsl(212_60%_22%),hsl(220_40%_5%))]" />
      ) : (
        images.map((image, position) => (
          <img
            alt=""
            className={`absolute inset-0 size-full object-cover transition-opacity duration-700 ${
              position === index ? "opacity-100" : "opacity-0"
            }`}
            key={image}
            src={image}
          />
        ))
      )}
      <div className="absolute inset-0 bg-gradient-to-r from-black/95 via-black/55 to-transparent" />
      <div className="absolute inset-0 bg-gradient-to-t from-black/85 via-transparent to-black/25" />
      <div className="relative flex h-full">
        <aside className="flex w-[240px] shrink-0 flex-col gap-4 px-8 py-10">
          <p className="text-sm font-semibold tracking-tight text-white/90">Peregrine</p>
          <h1 className="text-xs font-medium uppercase tracking-[0.22em] text-white/55">
            {t("profile.who")}
          </h1>
          <ul className="mt-2 flex list-none flex-col gap-4 p-0">
            {profiles.map((profile) => {
              const current = profile.id === activeId;
              return (
                <li className="flex items-center gap-3" key={profile.id}>
                  <button
                    aria-current={current}
                    aria-label={profile.name}
                    className={`shrink-0 rounded-md outline-none transition-transform hover:scale-[1.03] focus-visible:ring-2 focus-visible:ring-white motion-reduce:transition-none motion-reduce:hover:scale-100 ${
                      current ? "ring-2 ring-white" : "ring-1 ring-white/15"
                    }`}
                    onClick={() => {
                      onSelect(profile.id);
                      onClose();
                    }}
                    ref={(node) => {
                      if (node === null) avatarRefs.current.delete(profile.id);
                      else avatarRefs.current.set(profile.id, node);
                    }}
                    type="button"
                  >
                    <ProfileImage profile={profile} size={56} />
                  </button>
                  <span
                    className={`min-w-0 flex-1 truncate text-sm ${current ? "font-medium text-white" : "text-white/60"}`}
                  >
                    {profile.name}
                  </span>
                  {current ? (
                    <IconButton
                      className="text-white/60 hover:text-white"
                      icon="pencil"
                      label={t("profile.edit")}
                      onClick={() => {
                        setEditing({ mode: "edit", id: profile.id });
                      }}
                      variant="ghost"
                    />
                  ) : null}
                </li>
              );
            })}
            <li className="flex items-center gap-3">
              <button
                aria-label={t("profile.add")}
                className="grid size-14 shrink-0 place-items-center rounded-md border border-white/25 bg-black/20 text-2xl text-white/45 outline-none transition-colors hover:border-white/50 hover:text-white focus-visible:ring-2 focus-visible:ring-white"
                onClick={() => {
                  setEditing({ mode: "create" });
                }}
                type="button"
              >
                +
              </button>
              <span className="max-w-[7rem] text-[11px] leading-snug text-white/45">
                {t("profile.local")}
              </span>
            </li>
          </ul>
        </aside>
        <div className="min-w-0 flex-1" />
      </div>

      <ProfileEditor
        mode={editing?.mode ?? "create"}
        onClose={() => {
          setEditing(null);
        }}
        onSave={(name, avatar) => {
          if (editing?.mode === "edit") onRename(editing.id, name, avatar);
          else onAdd(name, avatar);
          setEditing(null);
        }}
        open={editing !== null}
        profile={edited}
      />
    </div>
  );
}
