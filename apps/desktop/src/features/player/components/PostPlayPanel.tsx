/**
 * The card that stands between the last frame and whatever happens next.
 *
 * **It is a projection, not a timer.** The phase, the remaining milliseconds, and the next-up all
 * come from `@peregrine/player-ui`'s coordinator, so this component cannot start a second episode,
 * restart a countdown, or decide that a cancelled offer is available again.
 *
 * **A film ends on the panel.** With no next episode there is nothing to count down to, so the card
 * offers Replay and a way back rather than leaving the element on a black rectangle.
 */

import type { PostPlayState } from "@peregrine/player-ui";
import { Button } from "@peregrine/ui";
import type { ReactElement } from "react";
import { useTranslation } from "react-i18next";

export interface PostPlayPanelProps {
  /** The coordinator's current answer. `hidden` draws nothing. */
  readonly state: PostPlayState;
  /** Hides the countdown and keeps the offer out of reach for this session. */
  readonly onCancel: () => void;
  /** Starts the offered episode now, without waiting for the countdown. */
  readonly onPlayNow: () => void;
  /** Seeks to the start and plays the current media again. */
  readonly onReplay: () => void;
  /** Leaves the player for the title page. */
  readonly onClose: () => void;
}

/** Whole seconds, rounded up so the last second is not shown as zero while it is still running. */
function countdownSeconds(remainingMs: number): number {
  if (!Number.isFinite(remainingMs) || remainingMs <= 0) {
    return 0;
  }
  return Math.ceil(remainingMs / 1000);
}

export function PostPlayPanel({
  state,
  onCancel,
  onPlayNow,
  onReplay,
  onClose,
}: PostPlayPanelProps): ReactElement | null {
  const { t } = useTranslation();

  if (state.phase === "hidden") {
    return null;
  }

  const next = state.nextUp;

  // A countdown with no offer is the film case the machine should never produce; drawing the ended
  // card anyway is what keeps a malformed state from becoming a panel with a Play now that does
  // nothing.
  if (state.phase === "ended" || next === null) {
    return (
      <div
        className="pointer-events-none absolute inset-0 z-20 flex items-center justify-center bg-surface-scrim p-4"
        data-player-post-play="ended"
      >
        <section
          aria-label={t("player.ended.title")}
          className="pointer-events-auto flex w-full max-w-sm flex-col gap-3 rounded-lg border border-border-subtle bg-surface-1 p-5 shadow-elev-3"
        >
          <p className="text-title-2 text-text-primary">{t("player.ended.title")}</p>
          <div className="flex justify-end gap-2">
            <Button onClick={onReplay} variant="primary">
              {t("player.ended.replay")}
            </Button>
            <Button onClick={onClose}>{t("player.back")}</Button>
          </div>
        </section>
      </div>
    );
  }

  const seconds = countdownSeconds(state.remainingMs);

  return (
    <div
      className="pointer-events-none absolute inset-0 z-20 flex items-center justify-center bg-surface-scrim p-4"
      data-player-post-play="countdown"
    >
      <section
        aria-label={t("player.nextUp.title")}
        className="pointer-events-auto flex w-full max-w-sm flex-col gap-3 rounded-lg border border-border-subtle bg-surface-1 p-5 shadow-elev-3"
      >
        <p className="text-title-2 text-text-primary">{t("player.nextUp.title")}</p>
        <p className="text-body text-text-secondary">{t("player.nextUp.countdown", { seconds })}</p>
        {next.name !== null ? (
          <p className="text-body-strong text-text-primary">{next.name}</p>
        ) : null}
        <div className="flex justify-end gap-2">
          <Button onClick={onCancel}>{t("player.nextUp.cancel")}</Button>
          <Button onClick={onPlayNow} variant="primary">
            {t("player.nextUp.playNow")}
          </Button>
        </div>
      </section>
    </div>
  );
}
