import { useMemo, type ReactElement } from "react";

import { avatarUri, initialsFor, type Profile } from "./profile.js";

export interface ProfileImageProps {
  readonly profile: Profile;
  /** The rendered edge in pixels; the generated avatar is asked for twice this so it stays crisp. */
  readonly size: number;
  readonly className?: string;
}

/**
 * A profile's picture: an uploaded image, a DiceBear drawing, or initials when neither can be drawn.
 *
 * The colour underneath is the profile's hue, so a transparent generated avatar still sits on
 * something rather than on the backdrop.
 */
export function ProfileImage({ profile, size, className }: ProfileImageProps): ReactElement {
  const src = useMemo(() => {
    try {
      return avatarUri(profile.avatar, size * 2);
    } catch {
      return null;
    }
  }, [profile.avatar, size]);

  return (
    <span
      className={`inline-grid shrink-0 place-items-center overflow-hidden rounded-md ${className ?? ""}`}
      style={{ backgroundColor: `hsl(${String(profile.hue)} 52% 42%)`, height: size, width: size }}
    >
      {src === null ? (
        <span className="text-sm font-semibold text-white">{initialsFor(profile.name)}</span>
      ) : (
        <img alt="" className="size-full object-cover" height={size} src={src} width={size} />
      )}
    </span>
  );
}
