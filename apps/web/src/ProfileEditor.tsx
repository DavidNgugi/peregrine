import { useEffect, useRef, useState } from "react";
import { createAvatar } from "@dicebear/core";
import {
  adventurer,
  avataaars,
  bigSmile,
  bottts,
  croodles,
  funEmoji,
  initials,
  lorelei,
  micah,
  notionists,
  openPeeps,
  personas,
  pixelArt,
  rings,
  shapes,
  thumbs,
} from "@dicebear/collection";

const styles = {
  initials,
  thumbs,
  bottts,
  funEmoji,
  notionists,
  adventurer,
  lorelei,
  shapes,
  avataaars,
  bigSmile,
  croodles,
  micah,
  openPeeps,
  personas,
  pixelArt,
  rings,
};
type Style = keyof typeof styles;

/** The desktop's labels, so the same style reads the same way on both screens. */
const labels: Record<Style, string> = {
  initials: "Initials",
  thumbs: "Thumbs",
  bottts: "Robots",
  funEmoji: "Faces",
  notionists: "Sketches",
  adventurer: "Adventurers",
  lorelei: "Portraits",
  shapes: "Shapes",
  avataaars: "Cartoons",
  bigSmile: "Smiles",
  croodles: "Doodles",
  micah: "Figures",
  openPeeps: "Peeps",
  personas: "Personas",
  pixelArt: "Pixels",
  rings: "Rings",
};

const CHOICES = 16;

/** Seeds derived from the name, so reopening the editor offers the same set. */
function seedsFor(name: string): string[] {
  const base = name.trim() === "" ? "You" : name.trim();
  return Array.from({ length: CHOICES }, (_, index) =>
    index === 0 ? base : `${base} ${String(index + 1)}`,
  );
}

/** Chromium 76 has no crypto.randomUUID, and the Tizen client runs this bundle. */
function randomSeeds(): string[] {
  const values = crypto.getRandomValues(new Uint32Array(CHOICES));
  return Array.from(values, (value) => value.toString(36).slice(0, 6));
}

function generated(style: Style, seed: string): string {
  return createAvatar(styles[style] as Parameters<typeof createAvatar>[0], {
    seed,
    size: 128,
    radius: 8,
  }).toDataUri();
}

export function ProfileEditor({
  initialName,
  initialAvatar,
  editing,
  onCancel,
  onSave,
}: {
  readonly initialName: string;
  readonly initialAvatar: string | null;
  readonly editing: boolean;
  readonly onCancel: () => void;
  readonly onSave: (name: string, avatar: string, kids: boolean) => Promise<void>;
}) {
  const [name, setName] = useState(initialName);
  const [style, setStyle] = useState<Style>("initials");
  const [seed, setSeed] = useState<string | null>(null);
  const [shuffled, setShuffled] = useState<string[] | null>(null);
  const [custom, setCustom] = useState(initialAvatar);
  const [kids, setKids] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const file = useRef<HTMLInputElement>(null);
  const nameInput = useRef<HTMLInputElement>(null);
  useEffect(() => {
    nameInput.current?.focus();
  }, []);
  const base = name.trim() === "" ? "You" : name.trim();
  const seeds = shuffled ?? seedsFor(base);
  const chosen = seed ?? base;
  const avatar = custom ?? generated(style, chosen);

  const upload = async (chosen: File): Promise<void> => {
    try {
      if (
        !["image/jpeg", "image/png", "image/webp", "image/gif"].includes(chosen.type) ||
        chosen.size > 12 * 1024 * 1024
      ) {
        throw new Error("Choose a JPG, PNG, WebP or GIF image smaller than 12 MB.");
      }
      const bitmap = await createImageBitmap(chosen);
      try {
        const side = Math.min(bitmap.width, bitmap.height);
        const canvas = document.createElement("canvas");
        canvas.width = 128;
        canvas.height = 128;
        const context = canvas.getContext("2d");
        if (context === null) throw new Error("This device cannot import images.");
        context.drawImage(
          bitmap,
          (bitmap.width - side) / 2,
          (bitmap.height - side) / 2,
          side,
          side,
          0,
          0,
          128,
          128,
        );
        setCustom(canvas.toDataURL("image/jpeg", 0.85));
        setError(null);
      } finally {
        bitmap.close();
      }
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : "The image could not be imported.");
    }
  };

  return (
    <form
      className="profile-editor"
      onSubmit={(event) => {
        event.preventDefault();
        if (busy || name.trim() === "") return;
        setBusy(true);
        setError(null);
        void onSave(name.trim(), avatar, kids)
          .catch((caught: unknown) => {
            setError(caught instanceof Error ? caught.message : "The profile could not be saved.");
          })
          .finally(() => setBusy(false));
      }}
    >
      <h2>{editing ? "Edit profile" : "Add a profile"}</h2>
      <div className="profile-editor-identity">
        <img alt="Avatar preview" src={avatar} width={112} height={112} />
        <label>
          Name
          <input
            ref={nameInput}
            data-focus="true"
            maxLength={40}
            value={name}
            onChange={(event) => setName(event.target.value)}
          />
        </label>
      </div>
      <button data-focus="true" type="button" onClick={() => file.current?.click()}>
        Upload an image
      </button>
      <input
        ref={file}
        hidden
        type="file"
        accept="image/jpeg,image/png,image/webp,image/gif"
        onChange={(event) => {
          const chosen = event.target.files?.[0];
          if (chosen !== undefined) void upload(chosen);
          event.target.value = "";
        }}
      />
      <div className="profile-style-options" aria-label="Avatar style">
        {(Object.keys(styles) as Style[]).map((value) => (
          <button
            data-focus="true"
            key={value}
            type="button"
            aria-pressed={custom === null && style === value}
            onClick={() => {
              setStyle(value);
              setCustom(null);
            }}
          >
            {labels[value]}
          </button>
        ))}
      </div>
      <div className="profile-avatar-head">
        <span>Choose an avatar</span>
        <button
          data-focus="true"
          type="button"
          onClick={() => {
            setShuffled(randomSeeds());
          }}
        >
          Shuffle
        </button>
      </div>
      <div className="profile-avatar-options" aria-label="Choose an avatar">
        {seeds.map((candidate) => (
          <button
            data-focus="true"
            type="button"
            aria-label={`${labels[style]} ${candidate}`}
            aria-pressed={custom === null && chosen === candidate}
            key={`${style}-${candidate}`}
            onClick={() => {
              setSeed(candidate);
              setCustom(null);
            }}
          >
            <img alt="" src={generated(style, candidate)} width={64} height={64} />
          </button>
        ))}
      </div>
      {!editing && (
        <label className="profile-kids">
          <input
            data-focus="true"
            type="checkbox"
            checked={kids}
            onChange={(event) => setKids(event.target.checked)}
          />
          Kids profile, which hides adult and unrated titles
        </label>
      )}
      {error !== null && (
        <p role="alert" className="profile-error">
          {error}
        </p>
      )}
      <div className="profile-editor-actions">
        <button data-focus="true" type="button" disabled={busy} onClick={onCancel}>
          Cancel
        </button>
        <button data-focus="true" type="submit" disabled={busy || name.trim() === ""}>
          {busy ? "Saving…" : "Save profile"}
        </button>
      </div>
      <small>Avatars by DiceBear</small>
    </form>
  );
}
