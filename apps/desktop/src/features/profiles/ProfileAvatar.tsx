import type { ReactElement } from "react";

import { ProfileImage } from "./ProfileImage.js";
import type { Profile } from "./profile.js";

export interface ProfileAvatarProps {
  readonly profile: Profile;
  /** The accessible name, which is the action ("Switch profile") rather than the person. */
  readonly label: string;
  readonly onClick: () => void;
  readonly size?: "sm" | "md" | "toolbar";
  /** The toolbar's buttons are circles; the sidebar keeps its square, so shape is the caller's. */
  readonly shape?: "square" | "circle";
  readonly current?: boolean;
}

/** The clickable profile mark, used by the sidebar and the Discover toolbar. */
export function ProfileAvatar({
  profile,
  label,
  onClick,
  size = "sm",
  shape = "square",
  current = false,
}: ProfileAvatarProps): ReactElement {
  const pixels = size === "sm" ? 36 : size === "toolbar" ? 44 : 56;
  return (
    <button
      aria-label={label}
      aria-pressed={current}
      className={`shrink-0 outline-none ring-offset-2 ring-offset-surface-1 transition-transform hover:scale-[1.03] focus-visible:ring-2 focus-visible:ring-accent motion-reduce:transition-none motion-reduce:hover:scale-100 ${shape === "circle" ? "rounded-full" : "rounded-md"}`}
      onClick={onClick}
      title={label}
      type="button"
    >
      <ProfileImage
        className={shape === "circle" ? "rounded-full" : "rounded-md"}
        profile={profile}
        size={pixels}
      />
    </button>
  );
}
