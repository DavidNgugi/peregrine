import type { ReactElement } from "react";
import { formatTimecode, progressFraction } from "./timecode.js";

/** What the shell knows about the player before the playback epic lands. */
export type PlaybackSurfaceState =
  | { readonly kind: "idle" }
  | {
      readonly kind: "playing";
      readonly title: string;
      readonly positionMs: number;
      readonly durationMs: number;
    };

export interface NowPlayingBarProps {
  readonly state: PlaybackSurfaceState;
  /** Localized idle copy; the package carries no messages of its own. */
  readonly idleLabel: string;
}

const BAR_CLASSES =
  "flex h-11 items-center justify-between gap-3 border-t border-border-subtle bg-surface-1 px-3";

/**
 * The player surface's bar. It persists across navigation, so it is the shell's
 * footer rather than part of any one screen (docs/ARCHITECTURE.md §6).
 */
export function NowPlayingBar({ state, idleLabel }: NowPlayingBarProps): ReactElement {
  if (state.kind === "idle") {
    return (
      <footer className={BAR_CLASSES} role="status" aria-live="polite">
        <span className="text-caption text-text-tertiary">{idleLabel}</span>
      </footer>
    );
  }

  const fraction = progressFraction(state.positionMs, state.durationMs);
  return (
    <footer className={BAR_CLASSES} role="status" aria-live="polite">
      <span className="truncate text-caption text-text-primary">{state.title}</span>
      <span className="text-caption tabular-nums text-text-tertiary">
        {formatTimecode(state.positionMs)} / {formatTimecode(state.durationMs)}
      </span>
      <div
        aria-label={state.title}
        aria-valuemax={100}
        aria-valuemin={0}
        aria-valuenow={Math.round(fraction * 100)}
        className="h-1 w-40 overflow-hidden rounded-full bg-surface-2"
        role="progressbar"
      >
        {/* Transform only: the bar never animates layout (AGENTS.md §6). */}
        <div
          className="h-full origin-left bg-accent"
          style={{ transform: `scaleX(${String(fraction)})` }}
        />
      </div>
    </footer>
  );
}
