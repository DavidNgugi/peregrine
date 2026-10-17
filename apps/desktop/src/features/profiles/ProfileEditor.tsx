import { useEffect, useRef, useState, type ReactElement } from "react";
import { useTranslation } from "react-i18next";

import { fileToAvatar } from "./avatar-file.js";
import { ProfileImage } from "./ProfileImage.js";
import {
  AVATAR_STYLE_KEYS,
  AVATAR_STYLE_LABELS,
  hueFor,
  type Avatar,
  type AvatarStyle,
  type Profile,
} from "./profile.js";

export interface ProfileEditorProps {
  readonly open: boolean;
  readonly mode: "create" | "edit";
  readonly profile?: Profile | undefined;
  readonly onSave: (name: string, avatar: Avatar) => void;
  readonly onClose: () => void;
}

const CHOICES = 16;

/** Sixteen seeds derived from a name, so opening the editor twice shows the same set. */
function seedsFor(name: string): string[] {
  const base = name.trim().length === 0 ? "Peregrine" : name.trim();
  return Array.from({ length: CHOICES }, (_, index) =>
    index === 0 ? base : `${base} ${String(index + 1)}`,
  );
}

function randomSeeds(): string[] {
  return Array.from({ length: CHOICES }, () => crypto.randomUUID().slice(0, 8));
}

/**
 * The profile editor, opened from the pencil or the plus tile.
 *
 * One screen: the picture and its upload on the left, the name on the right, then a style rail and
 * sixteen drawings of the current style. Shuffle draws sixteen new seeds, so the set is not limited
 * to the ones a name happens to produce.
 */
export function ProfileEditor({
  open,
  mode,
  profile,
  onSave,
  onClose,
}: ProfileEditorProps): ReactElement | null {
  const { t } = useTranslation();
  const [name, setName] = useState("");
  const [avatar, setAvatar] = useState<Avatar>({
    kind: "dicebear",
    style: "initials",
    seed: "You",
  });
  const [style, setStyle] = useState<AvatarStyle>("initials");
  const [seeds, setSeeds] = useState<string[]>(seedsFor("You"));
  const [error, setError] = useState(false);
  const file = useRef<HTMLInputElement>(null);
  const nameInput = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!open) return;
    const nextAvatar = profile?.avatar ?? { kind: "dicebear", style: "initials", seed: "You" };
    setName(profile?.name ?? "");
    setAvatar(nextAvatar);
    setStyle(nextAvatar.kind === "dicebear" ? nextAvatar.style : "initials");
    setSeeds(seedsFor(profile?.name ?? ""));
    setError(false);
    nameInput.current?.focus();
  }, [open, profile]);

  useEffect(() => {
    if (!open) return;
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
  }, [onClose, open]);

  if (!open) return null;

  const previewName = name.trim().length === 0 ? "Peregrine" : name.trim();
  const preview: Profile = {
    id: profile?.id ?? "preview",
    name: previewName,
    hue: hueFor(previewName),
    avatar,
  };

  const same = (choice: Avatar): boolean => {
    if (choice.kind !== avatar.kind) return false;
    if (choice.kind === "upload" && avatar.kind === "upload")
      return choice.dataUrl === avatar.dataUrl;
    return (
      choice.kind === "dicebear" &&
      avatar.kind === "dicebear" &&
      choice.style === avatar.style &&
      choice.seed === avatar.seed
    );
  };

  const save = (): void => {
    if (name.trim().length === 0) return;
    onSave(name.trim(), avatar);
  };

  return (
    <div
      aria-label={mode === "create" ? t("profile.newTitle") : t("profile.editTitle")}
      aria-modal="true"
      className="fixed inset-0 z-[60] grid place-items-center overflow-y-auto bg-black/80 p-6"
      role="dialog"
    >
      <div className="w-full max-w-3xl rounded-xl border border-white/10 bg-[#141414] p-6 text-white shadow-2xl">
        <h2 className="text-lg font-semibold">
          {mode === "create" ? t("profile.newTitle") : t("profile.editTitle")}
        </h2>

        <div className="mt-5 flex flex-wrap items-start gap-6">
          <div className="flex flex-col items-center gap-3">
            <ProfileImage
              className="rounded-lg ring-1 ring-white/15"
              profile={preview}
              size={112}
            />
            <button
              className="rounded-md border border-white/25 bg-black/30 px-3 py-1.5 text-xs text-white/80 outline-none transition-colors hover:border-white/60 hover:text-white focus-visible:ring-2 focus-visible:ring-white"
              onClick={() => {
                file.current?.click();
              }}
              type="button"
            >
              {t("profile.upload")}
            </button>
            <input
              accept="image/*"
              className="hidden"
              onChange={async (event) => {
                const chosen = event.target.files?.[0];
                if (chosen === undefined) return;
                try {
                  setAvatar(await fileToAvatar(chosen));
                  setError(false);
                } catch {
                  setError(true);
                } finally {
                  event.target.value = "";
                }
              }}
              ref={file}
              type="file"
            />
            {error ? <p className="text-xs text-red-400">{t("profile.uploadFailed")}</p> : null}
          </div>

          <label className="flex min-w-[16rem] flex-1 flex-col gap-2">
            <span className="text-xs font-medium uppercase tracking-[0.16em] text-white/50">
              {t("profile.nameLabel")}
            </span>
            <input
              className="rounded-md border border-white/20 bg-black/40 px-3 py-2 text-sm text-white outline-none focus:border-white/60"
              maxLength={40}
              onChange={(event) => {
                setName(event.target.value);
              }}
              onKeyDown={(event) => {
                if (event.key === "Enter") save();
              }}
              placeholder={t("profile.namePlaceholder")}
              ref={nameInput}
              value={name}
            />
            <span className="text-xs text-white/40">{t("profile.nameHint")}</span>
          </label>
        </div>

        <div className="mt-6 flex items-center justify-between gap-4">
          <p className="text-xs font-medium uppercase tracking-[0.16em] text-white/50">
            {t("profile.avatarHeading")}
          </p>
          <button
            className="rounded-md border border-white/20 px-3 py-1 text-xs text-white/70 outline-none hover:border-white/50 hover:text-white focus-visible:ring-2 focus-visible:ring-white"
            onClick={() => {
              setSeeds(randomSeeds());
            }}
            type="button"
          >
            {t("profile.shuffle")}
          </button>
        </div>

        <ul className="mt-3 flex list-none flex-wrap gap-1.5 p-0">
          {AVATAR_STYLE_KEYS.map((key) => (
            <li key={key}>
              <button
                aria-pressed={style === key}
                className={`rounded-full px-3 py-1 text-xs outline-none transition-colors focus-visible:ring-2 focus-visible:ring-white ${
                  style === key
                    ? "bg-white text-black"
                    : "border border-white/15 text-white/65 hover:border-white/40 hover:text-white"
                }`}
                onClick={() => {
                  setStyle(key);
                }}
                type="button"
              >
                {AVATAR_STYLE_LABELS[key]}
              </button>
            </li>
          ))}
        </ul>

        <ul className="mt-3 grid list-none grid-cols-8 gap-2 p-0">
          {seeds.map((seed) => {
            const choice: Avatar = { kind: "dicebear", style, seed };
            return (
              <li key={`${style}-${seed}`}>
                <button
                  aria-label={`${AVATAR_STYLE_LABELS[style]} ${seed}`}
                  aria-pressed={same(choice)}
                  className={`block w-full overflow-hidden rounded-md outline-none ring-2 ring-offset-2 ring-offset-[#141414] transition-transform hover:scale-[1.04] focus-visible:ring-white motion-reduce:hover:scale-100 ${
                    same(choice) ? "ring-white" : "ring-transparent"
                  }`}
                  onClick={() => {
                    setAvatar(choice);
                  }}
                  type="button"
                >
                  <ProfileImage
                    className="rounded-md"
                    profile={{ ...preview, avatar: choice }}
                    size={64}
                  />
                </button>
              </li>
            );
          })}
        </ul>

        <div className="mt-6 flex items-center justify-between gap-4 border-t border-white/10 pt-4">
          <span className="text-[11px] text-white/35">{t("profile.avatarsBy")}</span>
          <div className="flex gap-2">
            <button
              className="rounded-md border border-white/25 px-4 py-2 text-sm text-white/80 outline-none hover:border-white/60 hover:text-white focus-visible:ring-2 focus-visible:ring-white"
              onClick={onClose}
              type="button"
            >
              {t("profile.cancel")}
            </button>
            <button
              className="rounded-md bg-white px-4 py-2 text-sm font-medium text-black outline-none hover:bg-white/85 focus-visible:ring-2 focus-visible:ring-white disabled:cursor-not-allowed disabled:opacity-40"
              disabled={name.trim().length === 0}
              onClick={save}
              type="button"
            >
              {t("profile.save")}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
