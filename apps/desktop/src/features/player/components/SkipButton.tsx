/**
 * The one control that skips the window the viewer is already inside.
 *
 * It is a real `Button`, so Tab reaches it and Enter or Space operates it, and it is drawn over the
 * picture rather than in the chrome because the decision belongs to the frame being watched. Credits
 * never get one: a skip there would be a "skip the end" control, and the post-play panel is what
 * that moment offers instead.
 */

import type { PlayerMarker } from "@peregrine/player-ui";
import { Button } from "@peregrine/ui";
import type { ReactElement } from "react";
import { useTranslation } from "react-i18next";

/** The three marker kinds that can be skipped. */
export type SkippableMarker = PlayerMarker & {
  readonly kind: "intro" | "recap" | "preview";
};

/** The copy key for each skippable kind. */
const SKIP_LABEL_KEYS = {
  intro: "player.skip.intro",
  recap: "player.skip.recap",
  preview: "player.skip.preview",
} as const;

/** Whether a marker is one the viewer can skip past. */
export function isSkippableMarker(
  marker: PlayerMarker | null | undefined,
): marker is SkippableMarker {
  return marker !== null && marker !== undefined && marker.kind !== "credits";
}

export interface SkipButtonProps {
  /** The window the media position is currently inside. */
  readonly marker: SkippableMarker;
  /** Seeks to the marker's end and keeps playing. */
  readonly onSkip: (marker: SkippableMarker) => void;
}

export function SkipButton({ marker, onSkip }: SkipButtonProps): ReactElement {
  const { t } = useTranslation();

  return (
    // Above the element's own controls, and in the bottom-right corner where it cannot sit on the
    // subtitle band the sidecar is drawn in. The wrapper is inert to the pointer so only the button
    // itself takes the click.
    <div
      className="pointer-events-none absolute bottom-24 right-4 z-10"
      data-player-skip={marker.kind}
    >
      <Button
        className="pointer-events-auto bg-surface-0/90 shadow-elev-2"
        onClick={() => {
          onSkip(marker);
        }}
        variant="secondary"
      >
        {t(SKIP_LABEL_KEYS[marker.kind])}
      </Button>
    </div>
  );
}
