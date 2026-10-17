import { useEffect, useMemo, useRef, useState, type ReactElement } from "react";
import { useTranslation } from "react-i18next";

import { fileToAvatar } from "./avatar-file.js";
import { ProfileImage } from "./ProfileImage.js";
import {
  AVATAR_STYLE_KEYS,
  AVATAR_STYLE_LABELS,
  hueFor,
  type Avatar,
  type Profile,
} from "./profile.js";

export interface ProfileEditorProps {
  readonly open: boolean;
  readonly mode: "create" | "edit";
  readonly profile?: Profile | undefined;
  readonly onSave: (name: string, avatar: Avatar) => void;
  readonly onClose: () => void;
}

/**
 * The profile editor, opened from the pencil or the plus tile.
 *
 * One screen, two columns: the picture and its upload on the left, the name and the generated
 * choices on the right. The generated choices are DiceBear drawings over the current name and a
 * second seed, so a name change offers a different set without a refresh.
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
  const [error, setError] = useState(false);
  const file = useRef<HTMLInputElement>(null);
  const nameInput = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!open) return;
    setName(profile?.name ?? "");
    setAvatar(profile?.avatar ?? { kind: "dicebear", style: "initials", seed: "You" });
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

  const seed = name.trim().length === 0 ? "Peregrine" : name.trim();
  const suggestions = useMemo(
    () =>
      AVATAR_STYLE_KEYS.flatMap((style) => [
        { kind: "dicebear" as const, style, seed },
        { kind: "dicebear" as const, style, seed: `${seed} two` },
      ]),
    [seed],
  );

  if (!open) return null;

  const preview: Profile = {
    id: profile?.id ?? "preview",
    name: name.trim().length === 0 ? "Peregrine" : name.trim(),
    hue: hueFor(name.trim().length === 0 ? "Peregrine" : name.trim()),
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
      <div className="w-full max-w-2xl rounded-xl border border-white/10 bg-[#141414] p-6 text-white shadow-2xl">
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

        <p className="mt-6 text-xs font-medium uppercase tracking-[0.16em] text-white/50">
          {t("profile.avatarHeading")}
        </p>
        <ul className="mt-3 grid list-none grid-cols-8 gap-2 p-0">
          {suggestions.map((choice) => (
            <li key={`${choice.style}-${choice.seed}`}>
              <button
                aria-label={`${AVATAR_STYLE_LABELS[choice.style]} ${choice.seed}`}
                aria-pressed={same(choice)}
                className={`block w-full overflow-hidden rounded-md outline-none ring-2 ring-offset-2 ring-offset-[#141414] transition-transform hover:scale-[1.04] focus-visible:ring-white motion-reduce:hover:scale-100 ${
                  same(choice) ? "ring-white" : "ring-transparent"
                }`}
                onClick={() => {
                  setAvatar(choice);
                }}
                type="button"
              >
                <ProfileImage profile={{ ...preview, avatar: choice }} size={64} />
              </button>
            </li>
          ))}
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
