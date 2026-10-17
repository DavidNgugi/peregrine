import type { ReactElement } from "react";

import { initialsFor, type Profile } from "./profile.js";

export interface ProfileAvatarProps {
  readonly profile: Profile;
  /** The accessible name, which is the action ("Switch profile") rather than the person. */
  readonly label: string;
  readonly onClick: () => void;
  readonly size?: "sm" | "md";
  readonly current?: boolean;
}

/**
 * The profile's mark: initials on a colour derived from the name.
 *
 * A generated avatar rather than a photograph keeps the first run free of a file picker. DiceBear is
 * the intended long-term source, but it is a new dependency with per-style licences and the account
 * backend is not here yet; this is the stand-in with no network and no attribution to carry.
 */
export function ProfileAvatar({
  profile,
  label,
  onClick,
  size = "sm",
  current = false,
}: ProfileAvatarProps): ReactElement {
  const dimension = size === "sm" ? "size-9" : "size-14";
  return (
    <button
      aria-label={label}
      aria-pressed={current}
      className={`${dimension} grid shrink-0 place-items-center rounded-full text-xs font-semibold text-white outline-none ring-offset-2 ring-offset-surface-1 transition-transform hover:scale-[1.03] focus-visible:ring-2 focus-visible:ring-accent motion-reduce:transition-none motion-reduce:hover:scale-100`}
      onClick={onClick}
      style={{ backgroundColor: `hsl(${String(profile.hue)} 52% 42%)` }}
      title={label}
      type="button"
    >
      {initialsFor(profile.name)}
    </button>
  );
}
